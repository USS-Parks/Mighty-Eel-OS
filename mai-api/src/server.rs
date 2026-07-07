//! MAI server bootstrap: dual-stack REST + gRPC with graceful shutdown.
//!
//! `MaiServer` is the top-level entry point. It:
//! 1. Loads or defaults configuration
//! 2. Optionally runs the air-gap startup check
//! 3. Initializes all mai-core components (scheduler, registry, health,
//!    power, hotswap) and wraps them in shared `AppState`
//!    3b. Loads API keys from config/auth_keys.toml
//! 4. Starts the REST server (axum, default port 8420) and the gRPC
//!    server (tonic, default port 8421) concurrently
//! 5. Listens for SIGTERM / SIGINT (Unix) or ctrl-c (all platforms) and
//!    drains in-flight requests before exiting
//!
//! Both servers share the same `AppState` via `Arc`, so every component
//! (scheduler, registry, health monitor, power state machine, hotswap
//! manager, audit writer) is visible to both protocols.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::air_gap::{AirGapChecker, DevSwitchReader};
use crate::audit::{AuditWriter, MemoryAuditWriter};
use crate::audit_wal::{WalAuditConfig, WalAuditWriter};
use crate::auth::{self, AuthState};
use crate::config::{ProductTier, ServerConfig, load_or_default};
use crate::grpc::server::{GrpcServerConfig, build_grpc_server};
use crate::production_guard::{ProductionReadinessReport, RuntimeChecks, RuntimeOutcome};
use crate::routes::build_router;
use crate::sealer_builder::build_sealer;
use crate::ship_profile::{ProfileMode, ShipProfile, load_ship_profile};
use crate::state::{AppState, ShipReadiness};
use crate::trust_builder::{
    TrustComponents, TrustExchangeMode, build_trust_components, verify_boot_bundle,
};
use crate::vault_builder::build_vault;

use mai_compliance::audit::AuditLog as ComplianceAuditLog;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;
use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_hil::traits::AdapterConfig;
use mai_scheduler::{
    DefaultScheduler, GpuId, GpuTopology, HeuristicKvCacheManager, InstanceCapabilities,
    InstanceConfig, InstanceId, KvCacheConfig, SchedulerConfig as NewSchedulerConfig,
    ScoringConfig, TopologyConfig,
    metrics::{MetricsCollector, MetricsConfig},
};

/// Default path for auth keys config file.
const AUTH_KEYS_CONFIG_PATH: &str = "config/auth_keys.toml";
/// Default path for scheduler config when the server runs from the repo root.
const SCHEDULER_CONFIG_PATH: &str = "config/scheduler.toml";
/// Fallback scheduler config path when running from the workspace root.
const API_SCHEDULER_CONFIG_PATH: &str = "mai-api/config/scheduler.toml";
/// Default path for multi-factor scoring config.
const SCORING_CONFIG_PATH: &str = "config/scoring.toml";
/// Default path for KV cache config.
const KV_CONFIG_PATH: &str = "config/kv.toml";
/// Default path for GPU topology config.
const TOPOLOGY_CONFIG_PATH: &str = "config/topology.toml";

/// Parsed adapter configuration from adapters.toml.
#[derive(Debug, Clone, Default)]
pub struct AdapterBootConfig {
    /// Framework-level settings (heartbeat, timeouts, paths).
    pub framework: FrameworkConfig,
    /// Per-adapter settings keyed by adapter name.
    pub adapter_configs: HashMap<String, AdapterBootEntry>,
    /// Model alias map: user-facing name -> (adapter_name, backend_model).
    pub model_aliases: HashMap<String, (String, String)>,
}

/// Boot-time configuration for a single adapter.
#[derive(Debug, Clone)]
pub struct AdapterBootEntry {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub gpu_ids: Vec<u32>,
    pub max_concurrent: usize,
    pub models: Vec<String>,
}

/// Top-level MAI server. Owns the startup sequence and shutdown handle.
pub struct MaiServer {
    config: ServerConfig,
    config_path: Option<std::path::PathBuf>,
    adapter_config_path: Option<PathBuf>,
    /// SHIP-07: optional ship-profile TOML path. When set (programmatically
    /// or via the `MAI_SHIP_PROFILE` env var) the server bootstrap uses the
    /// SHIP-03/04/05/06 builders for vault / audit / sealer / trust and
    /// runs the production guard before binding sockets. When unset the
    /// legacy `StubVault` + `MemoryAuditWriter` defaults remain in effect
    /// for tests and local-dev bring-up.
    ship_profile_path: Option<PathBuf>,
}

/// Errors that can occur during server startup.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Air-gap verification failed: {0}")]
    AirGap(String),
    #[error("Component initialization failed: {0}")]
    Init(String),
    #[error("REST server bind failed: {0}")]
    RestBind(String),
    #[error("gRPC server error: {0}")]
    GrpcError(String),
    #[error("Unexpected runtime error: {0}")]
    Runtime(String),
}

impl MaiServer {
    /// Create a server instance from a config file path.
    /// Falls back to Scout tier defaults if the file cannot be loaded.
    pub fn from_config_path(path: &Path) -> Self {
        let config = load_or_default(Some(path), ProductTier::Scout);
        Self {
            config,
            config_path: Some(path.to_path_buf()),
            adapter_config_path: None,
            ship_profile_path: None,
        }
    }

    /// Create a server instance with an explicit configuration.
    pub fn with_config(config: ServerConfig) -> Self {
        Self {
            config,
            config_path: None,
            adapter_config_path: None,
            ship_profile_path: None,
        }
    }

    /// Create a server with default Scout tier configuration.
    pub fn default_scout() -> Self {
        let config = load_or_default(None, ProductTier::Scout);
        Self {
            config,
            config_path: None,
            adapter_config_path: None,
            ship_profile_path: None,
        }
    }

    /// Set the adapter configuration file path.
    #[must_use]
    pub fn with_adapter_config(mut self, path: PathBuf) -> Self {
        self.adapter_config_path = Some(path);
        self
    }

    /// Attach a ship-profile TOML so SHIP-03/04/05/06 builders are used
    /// during bootstrap. Equivalent to setting `MAI_SHIP_PROFILE` in the
    /// environment; an explicit setter is honoured first.
    #[must_use]
    pub fn with_ship_profile(mut self, path: PathBuf) -> Self {
        self.ship_profile_path = Some(path);
        self
    }

    /// Run the full startup sequence, block until shutdown signal.
    ///
    /// Startup order:
    /// 1. Validate configuration
    /// 2. Air-gap verification (if enforcement enabled)
    /// 3. Initialize mai-core components
    ///    3b. Load API key authentication
    /// 4. Build shared AppState
    /// 5. Start REST + gRPC servers concurrently
    /// 6. Block on shutdown signal (SIGTERM / SIGINT / ctrl-c)
    /// 7. Graceful drain (up to 5 seconds)
    #[allow(clippy::too_many_lines, clippy::print_stdout)]
    pub async fn run(self) -> Result<(), ServerError> {
        // WELCOME-01: print the lamprey ASCII banner to stdout before
        // any tracing logs, so the first thing a tester sees when they
        // launch `lamprey-mai-api.exe` directly is the project identity
        // rather than bare structured logs. Suppress with
        // `MAI_NO_BANNER=1` when the `lamprey-mai.exe` launcher already
        // printed it.
        if std::env::var_os("MAI_NO_BANNER").is_none() {
            const LAMPREY_BANNER: &str = include_str!("../../docs/assets/lamprey-banner.txt");
            println!("{LAMPREY_BANNER}");
            println!();
            println!("  Lamprey MAI   build {}", env!("CARGO_PKG_VERSION"));
            println!("  © 2026 Island Mountain — USS-Parks LLC");
            println!("  starting REST + gRPC; logs follow.");
            println!();
        }

        info!(
            tier = ?self.config.tier,
            rest_port = self.config.server.port,
            grpc_port = self.config.server.grpc_port,
            bind = %self.config.server.bind_address,
            "MAI server starting"
        );

        // -- Step 1: Validate configuration--
        self.config
            .validate()
            .map_err(|e| ServerError::Config(format!("Configuration validation failed: {e}")))?;

        // -- Step 2: Air-gap verification--
        if self.config.air_gap.enforce_on_startup {
            info!("Running air-gap startup verification");
            let reader = Arc::new(DevSwitchReader::new());
            let checker = AirGapChecker::with_default_interval(reader);
            match checker.verify().await {
                Ok(ref result) if result.air_gapped => {
                    info!("Air-gap verification passed");
                }
                Ok(_) => {
                    warn!(
                        "Air-gap verification: switch reports non-air-gapped state, \
                         proceeding (simulated reader)"
                    );
                }
                Err(e) => {
                    return Err(ServerError::AirGap(e));
                }
            }
        }

        // -- Step 3: Initialize mai-core components--

        // SHIP-07: resolve optional ship profile (programmatic field first,
        // then MAI_SHIP_PROFILE env var). When present, the SHIP-03..06
        // builders drive vault / audit / sealer / trust; otherwise the
        // legacy StubVault + MemoryAuditWriter defaults are kept for
        // tests and local-dev bring-up.
        let ship_profile = self.resolve_ship_profile()?;

        // Load scheduler, topology, KV, and multi-factor scoring config
        // before publishing the scheduler behind the trait object.
        let scheduler: Arc<dyn mai_scheduler::Scheduler> = Arc::new(build_configured_scheduler());

        // Vault: builder-driven when a ship profile is loaded, StubVault
        // otherwise so the no-profile bring-up path is unchanged. V2/V3: the
        // builder returns an *initialized* vault or an error — a failed
        // initialization aborts run() here, before any socket binds.
        let mut vault_probe: Option<RuntimeOutcome> = None;
        let vault_box: Box<dyn VaultInterface> = if let Some(profile) = ship_profile.as_ref() {
            let vault = build_vault(profile).await.map_err(|e| {
                ServerError::Init(format!("vault builder rejected ship profile: {e}"))
            })?;
            // V8: measure the vault before certifying it — a storage
            // round-trip, never an unconditional pass.
            vault_probe = Some(crate::vault_builder::probe_vault(vault.as_ref()).await);
            vault
        } else {
            Box::new(StubVault)
        };
        let registry = ModelRegistry::new(vault_box);
        let registry = Arc::new(RwLock::new(registry));

        let health = HealthMonitor::new(HealthConfig::default());
        let health = Arc::new(RwLock::new(health));

        let power = PowerStateMachine::new(PowerConfig::default());
        let power = Arc::new(RwLock::new(power));

        // HotSwapManager still uses the old mai-core scheduler internally for
        // adapter lifecycle coordination (pause_routing, resume_routing, etc.).
        // TODO(basho): migrate HotSwapManager to the new Scheduler trait
        // (health integration); until then wire a default legacy scheduler.
        let legacy_scheduler =
            mai_core::scheduler::Scheduler::new(mai_core::scheduler::SchedulerConfig::default())
                .map_err(|e| ServerError::Init(format!("Legacy scheduler for hotswap: {e}")))?;
        let legacy_scheduler = Arc::new(RwLock::new(legacy_scheduler));
        let hotswap = HotSwapManager::new(legacy_scheduler, registry.clone(), health.clone());
        let hotswap = Arc::new(RwLock::new(hotswap));

        // API audit writer: persistent WAL when a ship profile is loaded,
        // in-memory writer otherwise (test/dev fallback).
        let audit_writer: Arc<dyn AuditWriter> = if let Some(profile) = ship_profile.as_ref() {
            let wal_config = WalAuditConfig::for_dir(&profile.audit.wal_dir);
            let writer = WalAuditWriter::open(wal_config).await.map_err(|e| {
                ServerError::Init(format!(
                    "WAL audit writer failed to open at {}: {e}",
                    profile.audit.wal_dir.display()
                ))
            })?;
            Arc::new(writer)
        } else {
            Arc::new(MemoryAuditWriter::new())
        };
        let config = Arc::new(RwLock::new(self.config.clone()));

        // Step 3b: Load API key authentication --
        let auth = load_auth_state(ship_profile.as_ref())?;

        // -- Step 3c: Load adapter config and start AdapterManager --
        let adapter_boot = load_adapter_boot_config(self.adapter_config_path.as_deref());
        let adapter_manager = AdapterManager::new(adapter_boot.framework.clone());
        let adapter_manager = Arc::new(Mutex::new(adapter_manager));

        // Discover and start configured adapters
        {
            let mgr = adapter_manager.lock().await;

            // Discover adapters from the adapters directory
            match mgr.discover().await {
                Ok(discovered) => {
                    info!(count = discovered.len(), "Adapters discovered");
                }
                Err(e) => {
                    warn!(error = %e, "Adapter discovery failed, continuing without adapters");
                }
            }

            // Start each enabled adapter and register with scheduler
            for (name, entry) in &adapter_boot.adapter_configs {
                if !entry.enabled {
                    info!(adapter = %name, "Adapter disabled in config, skipping");
                    continue;
                }

                let adapter_cfg = AdapterConfig {
                    backend_name: name.clone(),
                    host: entry.host.clone(),
                    port: entry.port,
                    model_path: String::new(),
                    max_concurrent_requests: entry.max_concurrent,
                    timeout_ms: adapter_boot.framework.request_timeout_ms,
                    gpu_layers: None,
                    quantization: None,
                    extra: std::collections::HashMap::new(),
                };

                match mgr.start_adapter(name, adapter_cfg).await {
                    Ok(managed) => {
                        info!(
                            adapter = %name,
                            version = %managed.version,
                            handle = %managed.handle,
                            "Adapter started, registering with scheduler"
                        );

                        // Register each model served by this adapter as an
                        // instance in the new scheduler.
                        let gpu_ids: Vec<GpuId> =
                            entry.gpu_ids.iter().map(|id| GpuId::new(*id)).collect();

                        for model in &entry.models {
                            let instance_id = format!("{name}:{model}");
                            let instance_cfg = InstanceConfig {
                                id: InstanceId::new(&instance_id),
                                model_name: model.clone(),
                                adapter_type: name.clone(),
                                gpu_ids: gpu_ids.clone(),
                                #[allow(clippy::cast_possible_truncation)]
                                max_batch_size: entry.max_concurrent as u32,
                                vram_allocated: 0, // Populated by health monitor
                                capabilities: InstanceCapabilities::default(),
                            };
                            if let Err(e) = scheduler.register_instance(instance_cfg) {
                                warn!(
                                    instance = %instance_id,
                                    error = %e,
                                    "Failed to register instance with scheduler"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(adapter = %name, error = %e, "Failed to start adapter");
                    }
                }
            }
        }

        // -- Step 4: Build shared AppState--
        let metrics_collector = Arc::new(MetricsCollector::new(MetricsConfig::default()));
        let (auth_key_count, auth_bypass_runtime) = {
            let store = auth.key_store.read().await;
            (store.len(), store.allow_internal_profile_header)
        };
        let state = AppState::new(
            scheduler,
            registry,
            health,
            power,
            hotswap,
            audit_writer,
            config,
            auth,
            adapter_manager.clone(),
            metrics_collector,
        );

        // SEC-95: optional per-route rate limiter configured under
        // `ServerConfig.limits.route_rate_limits`.
        let state = match self
            .config
            .build_route_rate_limiter()
            .map_err(|e| ServerError::Config(e.to_string()))?
        {
            Some(limiter) => state.with_rate_limiter(Arc::new(limiter)),
            None => state,
        };

        // SHIP-07: when a ship profile is loaded, swap in the
        // sealer-backed compliance audit log and the real trust
        // verifier so the demo defaults never reach handlers.
        let (state, runtime_checks) = match ship_profile.as_ref() {
            Some(profile) => apply_ship_profile(
                state,
                profile,
                auth_key_count,
                auth_bypass_runtime,
                vault_probe,
            )?,
            None => (state, RuntimeChecks::default()),
        };

        // SHIP-07: production guard fails closed before any socket
        // binds. Runtime-introspection results upgrade the deferred
        // `PROD-*-100/101` IDs from Deferred to Pass / Fail.
        if let Some(profile) = ship_profile.as_ref() {
            let report = ProductionReadinessReport::evaluate_with_runtime(profile, &runtime_checks);
            if !report.is_ship_ready() {
                error!(
                    profile = %profile.profile.name,
                    "production readiness check failed; refusing to bind sockets"
                );
                return Err(ServerError::Init(format!(
                    "production guard failed:\n{}",
                    report.render_human()
                )));
            }
            info!(
                profile = %profile.profile.name,
                mode = ?profile.profile.mode,
                pass = report.counts().pass,
                deferred = report.counts().deferred,
                "production readiness check passed",
            );
        }

        info!("All components initialized, building servers");

        // -- Step 5: Start REST + gRPC servers--
        let rest_addr: SocketAddr = format!(
            "{}:{}",
            self.config.server.bind_address, self.config.server.port
        )
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid REST bind address: {e}")))?;

        let grpc_addr: SocketAddr = format!(
            "{}:{}",
            self.config.server.bind_address, self.config.server.grpc_port
        )
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid gRPC bind address: {e}")))?;

        let router = build_router(state.clone());
        let listener = TcpListener::bind(rest_addr)
            .await
            .map_err(|e| ServerError::RestBind(format!("{rest_addr}: {e}")))?;

        info!(addr = %rest_addr, "REST server listening");

        let grpc_config = GrpcServerConfig {
            bind_addr: grpc_addr,
            enable_reflection: true,
            ..GrpcServerConfig::default()
        };

        let grpc_future = build_grpc_server(state.clone(), grpc_config)
            .await
            .map_err(|e| ServerError::GrpcError(format!("Failed to build gRPC server: {e}")))?;

        info!(addr = %grpc_addr, "gRPC server listening");

        info!("MAI server ready — REST on {rest_addr}, gRPC on {grpc_addr}");

        // -- Step 6: Block on shutdown signal--
        let shutdown = shutdown_signal();

        tokio::select! {
            result = axum::serve(listener, router).with_graceful_shutdown(shutdown) => {
                if let Err(e) = result {
                    error!(error = %e, "REST server exited with error");
                    return Err(ServerError::Runtime(format!("REST: {e}")));
                }
            }
            result = grpc_future => {
                if let Err(e) = result {
                    error!(error = %e, "gRPC server exited with error");
                    return Err(ServerError::GrpcError(format!("{e}")));
                }
            }
        }

        // -- Step 7: Graceful drain--
        info!("Shutdown signal received, draining in-flight requests (5s max)");

        // Shut down all adapter processes cleanly
        {
            let mgr = adapter_manager.lock().await;
            if let Err(e) = mgr.shutdown_all().await {
                error!(error = %e, "Error during adapter shutdown");
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
        info!("MAI server shut down cleanly");

        Ok(())
    }
}

impl MaiServer {
    /// Resolve the ship profile if any: programmatic field first, then
    /// the `MAI_SHIP_PROFILE` environment variable. Returns `Ok(None)`
    /// when neither is set so the legacy bring-up path stays in force.
    fn resolve_ship_profile(&self) -> Result<Option<ShipProfile>, ServerError> {
        let path: Option<PathBuf> = self
            .ship_profile_path
            .clone()
            .or_else(|| std::env::var_os("MAI_SHIP_PROFILE").map(PathBuf::from));
        let Some(path) = path else {
            return Ok(None);
        };
        info!(path = %path.display(), "Loading ship profile");
        let profile = load_ship_profile(&path).map_err(|e| {
            ServerError::Config(format!("ship profile {} did not load: {e}", path.display()))
        })?;
        info!(
            profile = %profile.profile.name,
            mode = ?profile.profile.mode,
            "ship profile loaded; production builders active"
        );
        Ok(Some(profile))
    }
}

/// Build the sealer-backed compliance audit log and trust components,
/// install them onto `state`, and collect the runtime-introspection
/// results used by the production guard.
///
/// Returns the wired-up state plus the [`RuntimeChecks`] populated for
/// the configured profile. Failures bubble up as [`ServerError::Init`]
/// so production startup fails closed.
fn apply_ship_profile(
    state: AppState,
    profile: &ShipProfile,
    auth_key_count: usize,
    auth_bypass_runtime: bool,
    vault_probe: Option<RuntimeOutcome>,
) -> Result<(AppState, RuntimeChecks), ServerError> {
    let is_production = matches!(profile.profile.mode, ProfileMode::Production);

    // Sealer + compliance audit log.
    let sealer = build_sealer(profile)
        .map_err(|e| ServerError::Init(format!("sealer builder rejected ship profile: {e}")))?;
    let compliance_audit = ComplianceAuditLog::builder().sealer(sealer).build();

    // Trust components: bundle verifier + token-exchange mode.
    let TrustComponents {
        bundle_verifier,
        exchange_mode,
        anchor_ids,
    } = build_trust_components(profile)
        .map_err(|e| ServerError::Init(format!("trust builder rejected ship profile: {e}")))?;
    info!(
        anchors = anchor_ids.len(),
        exchange_mode = exchange_mode.label(),
        "trust components built"
    );

    // Wire the OpenBao bridge client when the exchange mode is
    // OpenBaoBridge. If the ship profile has an `[openbao]` section,
    // use it (secrets still come from env); otherwise fall back to
    // `::staging()` for no-profile / legacy bring-up.
    let bridge_client = if matches!(exchange_mode, TrustExchangeMode::OpenBaoBridge) {
        use crate::openbao_client::{OpenBaoBridgeClient, OpenBaoBridgeConfig};
        let config = if let Some(ref ob) = profile.openbao {
            OpenBaoBridgeConfig::from_profile(ob)
        } else {
            info!("no [openbao] section in ship profile, falling back to staging() config");
            OpenBaoBridgeConfig::staging()
        };
        let bridge = OpenBaoBridgeClient::new(config);
        info!("OpenBao bridge client wired");
        Some(bridge)
    } else {
        None
    };

    // Spawn background trust-refresh loop when the bridge is wired and
    // the profile's trust_refresh section is enabled.
    if let (Some(ob), Some(bridge)) = (&profile.openbao, &bridge_client) {
        if ob.trust_refresh.enabled {
            let cache = state.trust_cache.clone();
            let bridge_clone = bridge.clone();
            let failures = state.openbao_consecutive_failures.clone();
            let cancel = state.cancel_token.clone();
            let interval_secs = ob.trust_refresh.interval_secs;
            let interval = Duration::from_secs(interval_secs);
            let tenant_id = "tribal-health-demo".to_string();

            tokio::spawn(async move {
                info!(interval_secs, "background trust-refresh loop started");
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!("trust-refresh loop: cancellation received, shutting down");
                            break;
                        }
                        _ = tokio::time::sleep(interval) => {}
                    }

                    match bridge_clone.fetch_revocation_snapshots(&tenant_id).await {
                        Ok(snapshots) => {
                            if !snapshots.is_empty() {
                                cache.write().await.record_revocations(snapshots);
                            }
                            failures.store(0, std::sync::atomic::Ordering::Relaxed);
                            debug!("trust-refresh: snapshots ingested, failures reset");
                        }
                        Err(e) => {
                            let prev = failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let current = prev + 1;
                            if current >= 10 {
                                tracing::error!(
                                    failures = current,
                                    error = %e,
                                    "trust-refresh: OpenBao unreachable ({} consecutive failures)", current
                                );
                            } else if current >= 3 {
                                tracing::warn!(
                                    failures = current,
                                    "trust-refresh: OpenBao degraded ({} consecutive failures)",
                                    current
                                );
                            } else {
                                tracing::debug!(
                                    failures = current,
                                    "trust-refresh: transient OpenBao error"
                                );
                            }
                        }
                    }
                }
            });
        } else {
            info!("trust-refresh disabled by profile");
        }
    }

    // Boot bundle verification: required in production, skipped for
    // local-dev where bundles are typically not provisioned during
    // bring-up.
    let trust_outcome = if is_production && profile.trust.require_bundle_on_boot {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match verify_boot_bundle(profile, bundle_verifier.as_ref(), now) {
            Ok(version) => RuntimeOutcome::pass(format!(
                "bundle v{version} verified against {} anchors",
                anchor_ids.len()
            )),
            Err(e) => RuntimeOutcome::fail(format!("boot bundle verify: {e}")),
        }
    } else {
        RuntimeOutcome::pass(format!(
            "bundle verification not required ({:?})",
            profile.profile.mode
        ))
    };

    // PROD-AUDIT-101: compliance sealer is real (build_sealer rejects
    // null sealer in production; reaching this point means it is real
    // for any production profile).
    let sealer_outcome = RuntimeOutcome::pass(if is_production {
        "AEAD sealer wired from sealer.key".to_string()
    } else {
        "ephemeral AEAD sealer (local-dev)".to_string()
    });

    // V8: the vault outcome is MEASURED (storage round-trip + audit append,
    // run in `MaiServer::run` right after initialized construction) — never
    // fabricated. A missing probe fails closed.
    let vault_outcome = vault_probe.unwrap_or_else(|| {
        RuntimeOutcome::fail("vault readiness probe did not run (fail closed)".to_string())
    });

    let wal_outcome =
        RuntimeOutcome::pass(format!("WAL opened at {}", profile.audit.wal_dir.display()));

    let auth_outcome = if auth_key_count >= 1 {
        RuntimeOutcome::pass(format!("{auth_key_count} key(s) loaded"))
    } else {
        RuntimeOutcome::fail("auth key store is empty".to_string())
    };

    // SHIP-17 / PROD-AUTH-101: the runtime store's
    // `allow_internal_profile_header` flag must match the profile
    // field the static guard checked. Any divergence means the
    // X-IM-Internal-Profile bypass is live despite a profile that
    // declared it disabled (or vice versa); this is the gap
    // KNOWN-ISSUES Issue 13 was filed for.
    let profile_bypass = profile.auth.allow_internal_profile_header;
    let auth_bypass_outcome = if auth_bypass_runtime == profile_bypass {
        RuntimeOutcome::pass(format!(
            "runtime bypass = {auth_bypass_runtime}, profile field = {profile_bypass}: consistent"
        ))
    } else {
        RuntimeOutcome::fail(format!(
            "runtime bypass = {auth_bypass_runtime} but profile field = {profile_bypass}: \
             X-IM-Internal-Profile bypass diverges from profile contract"
        ))
    };

    // PolicyManager construction is infallible at this point — the
    // standard template loaded inside AppState::new succeeded.
    let policy_outcome = RuntimeOutcome::pass("standard policy modules loaded".to_string());

    let runtime = RuntimeChecks {
        vault_opened: Some(vault_outcome),
        api_audit_wal_ready: Some(wal_outcome),
        compliance_sealer_real: Some(sealer_outcome),
        trust_bundle_verified: Some(trust_outcome),
        auth_keys_nonempty: Some(auth_outcome),
        auth_internal_bypass_consistent: Some(auth_bypass_outcome),
        policy_modules_loaded: Some(policy_outcome),
    };

    // SHIP-07 Slice B: persist the exchange mode + readiness snapshot
    // on AppState so the profile-aware `exchange_token` handler and the
    // `/v1/system/production-readiness` endpoint have what they need.
    let readiness = ShipReadiness {
        profile: Arc::new(profile.clone()),
        runtime_checks: Arc::new(runtime.clone()),
    };
    let state = state
        .with_compliance_audit(compliance_audit)
        .with_bundle_verifier(bundle_verifier)
        .with_trust_exchange_mode(exchange_mode)
        .with_ship_readiness(readiness);

    let state = if let Some(bridge) = bridge_client {
        state.with_openbao_bridge(bridge)
    } else {
        state
    };

    Ok((state, runtime))
}

// Auth Loading --

/// Load authentication state from config or generate a first-boot key.
///
/// Path resolution (SHIP-17, closes KNOWN-ISSUES Issue 13):
/// 1. If `profile` is `Some`, read `profile.auth.auth_keys_path`.
/// 2. Otherwise, fall back to `AUTH_KEYS_CONFIG_PATH` (legacy no-profile
///    bring-up path, used by tests and dev runs without a ship profile).
///
/// Production failure semantics:
/// - Under `ProfileMode::Production`, a missing or unloadable keys file
///   is fatal: this function returns `ServerError::Init` and the
///   server refuses to bind. The first-boot path is forbidden in
///   production — the operator must provision the file before start.
/// - Under non-production modes, a missing keys file falls through to
///   the first-boot path. The runtime store's
///   `allow_internal_profile_header` flag inherits the profile field
///   (default `false`) so it can never silently diverge from the
///   value the production guard checked. With no profile at all, the
///   legacy dev default of `true` is preserved.
#[allow(clippy::print_stdout)]
fn load_auth_state(profile: Option<&ShipProfile>) -> Result<AuthState, ServerError> {
    let is_production = profile
        .map(|p| matches!(p.profile.mode, ProfileMode::Production))
        .unwrap_or(false);
    let auth_keys_pathbuf: PathBuf = profile
        .map(|p| p.auth.auth_keys_path.clone())
        .unwrap_or_else(|| PathBuf::from(AUTH_KEYS_CONFIG_PATH));
    let auth_path = auth_keys_pathbuf.as_path();

    if auth_path.exists() {
        match auth::load_api_keys_from_toml(auth_path) {
            Ok(store) => {
                info!(
                    keys = store.len(),
                    path = %auth_path.display(),
                    "API key authentication loaded from config"
                );
                return Ok(AuthState::with_key_store(store));
            }
            Err(e) => {
                if is_production {
                    return Err(ServerError::Init(format!(
                        "auth keys file at {} failed to load under production profile: {e}; \
                         first-boot fallback is forbidden in production",
                        auth_path.display()
                    )));
                }
                warn!(
                    error = %e,
                    path = %auth_path.display(),
                    "Failed to load auth config, falling back to first-boot mode"
                );
            }
        }
    } else if is_production {
        return Err(ServerError::Init(format!(
            "auth keys file missing at {} under production profile; \
             provision the file before start (first-boot fallback is forbidden in production)",
            auth_path.display()
        )));
    }

    // First-boot: generate an admin key and print it.
    // The admin copies this key into the configured auth_keys.toml
    // (hashed) for persistent authentication.
    let admin_key = auth::generate_api_key();
    let admin_hash = auth::hash_api_key(&admin_key);

    // Print to stdout so the admin can capture it. This is the ONLY
    // time the raw key is visible. It is never logged to the tracing
    // subscriber (which may write to disk).
    println!("========================================");
    println!("  MAI FIRST-BOOT: Admin API Key");
    println!("========================================");
    println!("  Key:  {admin_key}");
    println!("  Hash: {admin_hash}");
    println!();
    println!("  Save the KEY somewhere safe. Add the HASH");
    println!("  to {} to persist it:", auth_path.display());
    println!();
    println!("  [[keys]]");
    println!("  hash = \"{admin_hash}\"");
    println!("  profile_id = \"admin\"");
    println!("  role = \"admin\"");
    println!("  display_name = \"System Admin\"");
    println!("========================================");

    info!("First-boot admin key generated (printed to stdout, NOT logged)");

    // SHIP-17: the runtime store's `allow_internal_profile_header`
    // flag must match the profile field the production guard checks
    // (`PROD-AUTH-002`). When a profile is present we mirror its
    // value; with no profile we keep the legacy dev default of `true`
    // for the no-profile bring-up path.
    let bypass = profile
        .map(|p| p.auth.allow_internal_profile_header)
        .unwrap_or(true);
    let mut store = auth::ApiKeyStore::new();
    store.allow_internal_profile_header = bypass;
    store.add_key_hashed(
        admin_hash,
        "admin".to_string(),
        crate::types::ProfileRole::Admin,
        Some("First-Boot Admin".to_string()),
    );
    Ok(AuthState::with_key_store(store))
}

/// Wait for a shutdown signal: SIGTERM, SIGINT (Unix), or ctrl-c.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => { info!("ctrl-c received"); }
        () = terminate => { info!("SIGTERM received"); }
    }
}

/// Load adapter boot configuration from a TOML file.
///
/// If no path is provided, returns defaults (no adapters configured).
/// If the file cannot be read, logs a warning and returns defaults.
#[allow(clippy::manual_unwrap_or_default)]
fn explicit_vec_or_empty<T>(values: Option<Vec<T>>) -> Vec<T> {
    match values {
        Some(values) => values,
        None => Vec::new(),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn load_adapter_boot_config(path: Option<&Path>) -> AdapterBootConfig {
    let path = if let Some(p) = path {
        p
    } else {
        // Try the default location relative to the working directory
        let default_path = Path::new("config/adapters.toml");
        if default_path.exists() {
            default_path
        } else {
            info!("No adapter config file found, starting without adapters");
            return AdapterBootConfig::default();
        }
    };

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read adapter config, using defaults");
            return AdapterBootConfig::default();
        }
    };

    let table: toml::Table = match toml::from_str(&content) {
        Ok(t) => t,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Invalid adapter config TOML, using defaults");
            return AdapterBootConfig::default();
        }
    };

    // Parse framework settings
    let framework = FrameworkConfig::from_toml(path).unwrap_or_else(|e| {
        warn!(path = %path.display(), error = %e, "Invalid adapter framework config, using defaults");
        FrameworkConfig::default()
    });

    // Parse per-adapter configs from [adapters.*] sections
    let mut adapter_configs = HashMap::new();
    if let Some(adapters_section) = table.get("adapters").and_then(|v| v.as_table()) {
        for (name, adapter_table) in adapters_section {
            let Some(at) = adapter_table.as_table() else {
                continue;
            };
            let entry = AdapterBootEntry {
                enabled: at
                    .get("enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                host: at
                    .get("host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1")
                    .to_string(),
                port: at
                    .get("port")
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(11434) as u16,
                gpu_ids: explicit_vec_or_empty(at.get("gpu_ids").and_then(|v| v.as_array()).map(
                    |arr| {
                        arr.iter()
                            .filter_map(|v| v.as_integer().map(|i| i as u32))
                            .collect()
                    },
                )),
                max_concurrent: at
                    .get("max_concurrent")
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(4) as usize,
                models: explicit_vec_or_empty(at.get("models").and_then(|v| v.as_array()).map(
                    |arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                            .collect()
                    },
                )),
            };
            adapter_configs.insert(name.clone(), entry);
        }
    }

    // Parse model aliases from [model_aliases] section
    let mut model_aliases = HashMap::new();
    if let Some(aliases_section) = table.get("model_aliases").and_then(|v| v.as_table()) {
        for (alias, value) in aliases_section {
            if let Some(alias_table) = value.as_table() {
                let adapter = alias_table
                    .get("adapter")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let model = alias_table
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !adapter.is_empty() && !model.is_empty() {
                    model_aliases.insert(alias.clone(), (adapter, model));
                }
            }
        }
    }

    info!(
        adapters = adapter_configs.len(),
        aliases = model_aliases.len(),
        "Loaded adapter boot configuration"
    );

    AdapterBootConfig {
        framework,
        adapter_configs,
        model_aliases,
    }
}

/// Build the production scheduler with startup config wiring.
///
/// activates multi-factor scoring here: topology and KV handles are
/// attached before `config/scoring.toml` is applied, so scorer rebuild captures
/// every runtime dependency.
fn build_configured_scheduler() -> DefaultScheduler {
    let scheduler_config = load_scheduler_config();
    let topology = load_gpu_topology();
    let kv_manager: Arc<dyn mai_scheduler::KvCacheManager> =
        Arc::new(HeuristicKvCacheManager::new(load_kv_config()));

    let mut scheduler = DefaultScheduler::with_topology(scheduler_config, topology);
    scheduler.set_kv_manager(kv_manager);

    if let Some(scoring_config) = load_scoring_config() {
        scheduler.set_scoring_config(scoring_config);
        info!("Multi-factor scheduler scoring activated from config");
    } else {
        warn!("No scoring config found; scheduler will use least-loaded scoring");
    }

    scheduler
}

/// Load scheduler configuration from config/scheduler.toml.
///
/// Falls back to defaults if the file is missing or invalid.
/// Aliases are loaded from the `[aliases]` section.
fn load_scheduler_config() -> NewSchedulerConfig {
    let path = resolve_config_path(SCHEDULER_CONFIG_PATH, Some(API_SCHEDULER_CONFIG_PATH));
    if !path.exists() {
        info!("No scheduler config file found, using defaults");
        return NewSchedulerConfig::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read scheduler config, using defaults");
            return NewSchedulerConfig::default();
        }
    };

    match toml::from_str::<NewSchedulerConfig>(&content) {
        Ok(config) => {
            info!(
                strategy = %config.strategy,
                aliases = config.aliases.len(),
                overload_threshold = config.overload_queue_threshold,
                max_queue = config.max_total_queue_depth,
                "Loaded scheduler configuration"
            );
            config
        }
        Err(e) => {
            warn!(error = %e, "Invalid scheduler config TOML, using defaults");
            NewSchedulerConfig::default()
        }
    }
}

/// Load multi-factor scoring config from config/scoring.toml.
///
/// The checked-in file uses a `[scoring]` wrapper so future config files can
/// grow sibling sections without changing the scheduler crate's config type.
fn load_scoring_config() -> Option<ScoringConfig> {
    load_scoring_config_from_path(Path::new(SCORING_CONFIG_PATH))
}

#[derive(serde::Deserialize)]
struct ScoringConfigFile {
    scoring: ScoringConfig,
}

fn load_scoring_config_from_path(path: &Path) -> Option<ScoringConfig> {
    if !path.exists() {
        return None;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read scoring config");
            return None;
        }
    };

    match toml::from_str::<ScoringConfigFile>(&content) {
        Ok(file) => {
            info!(
                path = %path.display(),
                latency_weight = file.scoring.latency_weight,
                memory_weight = file.scoring.memory_weight,
                topology_weight = file.scoring.topology_weight,
                eviction_weight = file.scoring.eviction_weight,
                batching_weight = file.scoring.batching_weight,
                "Loaded multi-factor scoring configuration"
            );
            Some(file.scoring)
        }
        Err(wrapper_error) => match toml::from_str::<ScoringConfig>(&content) {
            Ok(config) => {
                info!(path = %path.display(), "Loaded direct scoring configuration");
                Some(config)
            }
            Err(direct_error) => {
                warn!(
                    path = %path.display(),
                    wrapper_error = %wrapper_error,
                    direct_error = %direct_error,
                    "Invalid scoring config TOML, leaving scorer unchanged"
                );
                None
            }
        },
    }
}

/// Load KV cache config and fall back to conservative defaults.
fn load_kv_config() -> KvCacheConfig {
    load_kv_config_from_path(Path::new(KV_CONFIG_PATH))
}

fn load_kv_config_from_path(path: &Path) -> KvCacheConfig {
    if !path.exists() {
        info!("No KV config file found, using defaults");
        return KvCacheConfig::default();
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read KV config, using defaults");
            return KvCacheConfig::default();
        }
    };

    match toml::from_str::<KvCacheConfig>(&content) {
        Ok(config) => {
            info!(
                path = %path.display(),
                budget_bytes = config.total_budget_bytes,
                model_factors = config.model_factors.len(),
                "Loaded KV cache configuration"
            );
            config
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Invalid KV config TOML, using defaults");
            KvCacheConfig::default()
        }
    }
}

/// Load topology config and discover the GPU topology.
fn load_gpu_topology() -> Arc<GpuTopology> {
    let config = load_topology_config_from_path(Path::new(TOPOLOGY_CONFIG_PATH));
    match GpuTopology::discover(&config) {
        Ok(topology) => {
            let topology = Arc::new(topology);
            info!(
                gpus = topology.gpu_count(),
                nvlink_cliques = topology.nvlink_cliques().len(),
                "GPU topology loaded"
            );
            topology
        }
        Err(e) => {
            warn!(error = %e, "GPU topology discovery failed, using flat topology");
            Arc::new(GpuTopology::flat(&TopologyConfig::default()))
        }
    }
}

fn load_topology_config_from_path(path: &Path) -> TopologyConfig {
    if !path.exists() {
        info!("No topology config file found, using defaults");
        return TopologyConfig::default();
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read topology config, using defaults");
            return TopologyConfig::default();
        }
    };

    match toml::from_str::<TopologyConfig>(&content) {
        Ok(config) => {
            info!(path = %path.display(), "Loaded topology configuration");
            config
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Invalid topology config TOML, using defaults");
            TopologyConfig::default()
        }
    }
}

fn resolve_config_path(primary: &str, fallback: Option<&str>) -> PathBuf {
    let primary = PathBuf::from(primary);
    if primary.exists() {
        return primary;
    }

    if let Some(fallback) = fallback {
        let fallback = PathBuf::from(fallback);
        if fallback.exists() {
            return fallback;
        }
    }

    primary
}

/// Stub vault implementation for server bootstrap.
///
/// In production, provides a real ZFS-backed vault. This stub
/// allows the server to start and pass health checks without vault hardware.
struct StubVault;

#[async_trait::async_trait]
impl VaultInterface for StubVault {
    async fn load_model_weights(
        &self,
        model_id: &str,
    ) -> Result<Vec<u8>, mai_core::vault::VaultError> {
        Err(mai_core::vault::VaultError::ModelNotFound(
            model_id.to_string(),
        ))
    }

    async fn store_model_package(
        &self,
        _model_id: &str,
        _data: &[u8],
    ) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }

    async fn verify_signature(
        &self,
        _data: &[u8],
        _signature: &[u8],
    ) -> Result<bool, mai_core::vault::VaultError> {
        Ok(true)
    }
}

// -- Tests--

#[cfg(test)]
mod tests {
    use super::*;
    use mai_scheduler::Scheduler;

    fn temp_config_path(name: &str) -> PathBuf {
        let unique = format!("mai-{name}-{}.toml", uuid::Uuid::new_v4());
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn test_default_scout_creates_server() {
        let server = MaiServer::default_scout();
        assert_eq!(server.config.server.port, 8420);
        assert_eq!(server.config.server.grpc_port, 8421);
        assert_eq!(server.config.server.bind_address, "127.0.0.1");
        assert!(server.adapter_config_path.is_none());
    }

    #[test]
    fn test_with_config_accepts_custom() {
        let mut config = ServerConfig::default();
        config.server.port = 9090;
        config.server.grpc_port = 9091;
        let server = MaiServer::with_config(config);
        assert_eq!(server.config.server.port, 9090);
        assert_eq!(server.config.server.grpc_port, 9091);
    }

    #[test]
    fn test_with_adapter_config() {
        let server =
            MaiServer::default_scout().with_adapter_config(PathBuf::from("config/adapters.toml"));
        assert_eq!(
            server.adapter_config_path.as_deref(),
            Some(Path::new("config/adapters.toml"))
        );
    }

    #[test]
    fn test_load_adapter_boot_config_missing_file() {
        let boot = load_adapter_boot_config(Some(Path::new("/nonexistent/path.toml")));
        assert!(boot.adapter_configs.is_empty());
        assert!(boot.model_aliases.is_empty());
    }

    #[test]
    fn test_stub_vault_returns_not_found() {
        let vault = StubVault;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(vault.load_model_weights("test-model"));
        assert!(result.is_err());
    }

    #[test]
    fn test_server_error_display() {
        let err = ServerError::Config("bad port".to_string());
        assert!(err.to_string().contains("bad port"));
        let err = ServerError::AirGap("switch offline".to_string());
        assert!(err.to_string().contains("switch offline"));
    }

    #[test]
    fn test_load_scoring_config_from_wrapped_toml() {
        let path = temp_config_path("scoring-wrapped");
        std::fs::write(
            &path,
            r#"
[scoring]
latency_weight = 3.25
memory_weight = 1.0
topology_weight = 2.0
eviction_weight = 0.5
batching_weight = 4.0
continuation_bonus = 8.0
"#,
        )
        .unwrap();

        let config = load_scoring_config_from_path(&path).unwrap();
        assert!((config.latency_weight - 3.25).abs() < f64::EPSILON);
        assert!((config.batching_weight - 4.0).abs() < f64::EPSILON);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_load_scoring_config_accepts_direct_toml() {
        let path = temp_config_path("scoring-direct");
        std::fs::write(
            &path,
            r#"
latency_weight = 0.5
memory_weight = 6.0
topology_weight = 0.0
eviction_weight = 0.0
batching_weight = 0.0
continuation_bonus = 0.0
"#,
        )
        .unwrap();

        let config = load_scoring_config_from_path(&path).unwrap();
        assert!((config.memory_weight - 6.0).abs() < f64::EPSILON);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_load_kv_and_topology_configs_from_files() {
        let kv_path = temp_config_path("kv");
        std::fs::write(&kv_path, "total_budget_bytes = 123456\n").unwrap();
        let kv = load_kv_config_from_path(&kv_path);
        assert_eq!(kv.total_budget_bytes, 123456);
        let _ = std::fs::remove_file(kv_path);

        let topology_path = temp_config_path("topology");
        std::fs::write(
            &topology_path,
            r#"
latency_weight = 2.0
bw_weight = 3.0
refresh_interval_ms = 250
"#,
        )
        .unwrap();
        let topology = load_topology_config_from_path(&topology_path);
        assert!((topology.latency_weight - 2.0).abs() < f64::EPSILON);
        assert_eq!(topology.refresh_interval_ms, 250);
        let _ = std::fs::remove_file(topology_path);
    }

    #[test]
    fn test_build_configured_scheduler_activates_runtime_handles() {
        let scheduler = build_configured_scheduler();

        assert!(scheduler.topology().is_some());
        assert!(scheduler.kv_manager().is_some());

        scheduler
            .register_instance(InstanceConfig {
                id: InstanceId::new("ollama:llama3"),
                model_name: "llama3".to_string(),
                adapter_type: "ollama".to_string(),
                gpu_ids: vec![GpuId::new(0)],
                max_batch_size: 4,
                vram_allocated: 8_000_000_000,
                capabilities: InstanceCapabilities::default(),
            })
            .unwrap();

        let decision = scheduler
            .schedule(&mai_scheduler::ScheduleRequest::new(
                "lamprey/fast",
                mai_scheduler::Priority::Normal,
            ))
            .unwrap();

        assert_eq!(decision.instance_id, InstanceId::new("ollama:llama3"));
    }

    #[test]
    fn test_load_auth_state_no_config() {
        // Legacy no-profile bring-up path: when no ship profile is
        // supplied and the default AUTH_KEYS_CONFIG_PATH does not
        // exist, load_auth_state generates a first-boot key and
        // returns a working AuthState with the dev bypass on. SHIP-17
        // preserves this behavior for the no-profile case so existing
        // dev/test runs are unaffected.
        let auth = load_auth_state(None).expect("no-profile first-boot must not fail");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let store = auth.key_store.read().await;
            assert_eq!(store.len(), 1);
            assert!(store.allow_internal_profile_header);
        });
    }

    // ---- SHIP-17 / KNOWN-ISSUES Issue 13 regression coverage ----

    /// Baseline production TOML with an `auth_keys_path` that points at
    /// a definitely-non-existent location. Built so SHIP-01 parsing
    /// accepts it (allow_internal_profile_header = false, non-empty
    /// path) and the guard's static checks pass; only the runtime
    /// load step should fail.
    fn ship17_baseline_toml(auth_keys_path: &str) -> String {
        format!(
            r#"
[profile]
name = "ship17-test"
mode = "production"
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"
require_sealed_master_key = true
require_pqc = true
allow_stub = false

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"
require_hash_chain = true
require_pqc_checkpoints = true
require_encryption_at_rest = true
allow_memory_writer = false
allow_null_sealer = false

[trust]
anchors_dir = "/etc/mai/trust-anchors"
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"
allow_accept_all_verifier = false
allow_local_dev_exchange = false
require_trust_anchor = true
require_bundle_on_boot = true

[auth]
auth_keys_path = "{auth_keys_path}"
allow_internal_profile_header = false
require_nonempty_key_store = true

[dashboard]
enabled = true
allow_default_admin_token = false

[network]
bind_address = "127.0.0.1"
tls_mode = "reverse-proxy-required"
require_forwarded_proto_header = false

[observability]
log_format = "json"
log_rotation = true
metrics_exporter = "prometheus"
alerts_enabled = true
"#
        )
    }

    #[test]
    fn load_auth_state_production_missing_file_fails_closed() {
        // SHIP-17 contract: under ProfileMode::Production, a missing
        // auth_keys_path is fatal. The first-boot fallback (which
        // would silently enable the X-IM-Internal-Profile bypass)
        // must not run.
        let toml = ship17_baseline_toml("/nonexistent/ship17/missing-auth-keys-prod.toml");
        let profile = crate::ship_profile::parse_ship_profile(&toml)
            .expect("ship17 baseline production toml parses");
        match load_auth_state(Some(&profile)) {
            Ok(_) => panic!(
                "production + missing auth_keys file must fail closed, but load_auth_state returned Ok"
            ),
            Err(ServerError::Init(msg)) => {
                assert!(
                    msg.contains("missing"),
                    "expected 'missing' in error, got: {msg}"
                );
                assert!(
                    msg.contains("first-boot fallback is forbidden in production"),
                    "expected fallback-forbidden message, got: {msg}"
                );
            }
            Err(other) => panic!("expected ServerError::Init, got {other:?}"),
        }
    }

    #[test]
    fn load_auth_state_non_production_first_boot_mirrors_profile_field() {
        // SHIP-17 contract: under non-production mode, a missing
        // auth_keys_path falls through to first-boot, but the runtime
        // store's allow_internal_profile_header inherits the profile
        // field so the two can never diverge (which would defeat
        // PROD-AUTH-002's static check).
        let toml = ship17_baseline_toml("/nonexistent/ship17/missing-auth-keys-dev.toml")
            .replace("mode = \"production\"", "mode = \"local-dev\"");
        let profile = crate::ship_profile::parse_ship_profile(&toml)
            .expect("ship17 baseline local-dev toml parses");
        // The parsed baseline has allow_internal_profile_header = false.
        let auth =
            load_auth_state(Some(&profile)).expect("non-production first-boot must not fail");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let store = auth.key_store.read().await;
            assert_eq!(store.len(), 1, "first-boot must seed one admin key");
            assert!(
                !store.allow_internal_profile_header,
                "runtime bypass must mirror profile field (false), not silently flip to true"
            );
        });
    }
}
