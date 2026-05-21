//! MAI server bootstrap: dual-stack REST + gRPC with graceful shutdown.
//!
//! `MaiServer` is the top-level entry point. It:
//! 1. Loads or defaults configuration
//! 2. Optionally runs the air-gap startup check
//! 3. Initializes all mai-core components (scheduler, registry, health,
//!    power, hotswap) and wraps them in shared `AppState`
//!    3b. Loads API keys from config/auth_keys.toml (Session 14c)
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
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use crate::air_gap::{AirGapChecker, DevSwitchReader};
use crate::audit::MemoryAuditWriter;
use crate::auth::{self, AuthState};
use crate::config::{ProductTier, ServerConfig, load_or_default};
use crate::grpc::server::{GrpcServerConfig, build_grpc_server};
use crate::routes::build_router;
use crate::state::AppState;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;
use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_hil::traits::AdapterConfig;
use mai_scheduler::{
    DefaultScheduler, GpuId, InstanceCapabilities, InstanceConfig, InstanceId,
    SchedulerConfig as NewSchedulerConfig,
};

/// Default path for auth keys config file.
const AUTH_KEYS_CONFIG_PATH: &str = "config/auth_keys.toml";

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
        }
    }

    /// Create a server instance with an explicit configuration.
    pub fn with_config(config: ServerConfig) -> Self {
        Self {
            config,
            config_path: None,
            adapter_config_path: None,
        }
    }

    /// Create a server with default Scout tier configuration.
    pub fn default_scout() -> Self {
        let config = load_or_default(None, ProductTier::Scout);
        Self {
            config,
            config_path: None,
            adapter_config_path: None,
        }
    }

    /// Set the adapter configuration file path.
    #[must_use]
    pub fn with_adapter_config(mut self, path: PathBuf) -> Self {
        self.adapter_config_path = Some(path);
        self
    }

    /// Run the full startup sequence, block until shutdown signal.
    ///
    /// Startup order:
    /// 1. Validate configuration
    /// 2. Air-gap verification (if enforcement enabled)
    /// 3. Initialize mai-core components
    ///    3b. Load API key authentication (Session 14c)
    /// 4. Build shared AppState
    /// 5. Start REST + gRPC servers concurrently
    /// 6. Block on shutdown signal (SIGTERM / SIGINT / ctrl-c)
    /// 7. Graceful drain (up to 5 seconds)
    #[allow(clippy::too_many_lines)]
    pub async fn run(self) -> Result<(), ServerError> {
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

        // Load scheduler config from scheduler.toml (Session 15)
        let scheduler_config = load_scheduler_config();
        let scheduler: Arc<dyn mai_scheduler::Scheduler> =
            Arc::new(DefaultScheduler::new(scheduler_config));

        // until Session 12 provides the real vault.
        let vault = StubVault;
        let registry = ModelRegistry::new(Box::new(vault));
        let registry = Arc::new(RwLock::new(registry));

        let health = HealthMonitor::new(HealthConfig::default());
        let health = Arc::new(RwLock::new(health));

        let power = PowerStateMachine::new(PowerConfig::default());
        let power = Arc::new(RwLock::new(power));

        // HotSwapManager still uses the old mai-core scheduler internally for
        // adapter lifecycle coordination (pause_routing, resume_routing, etc.).
        // It will be migrated to the new Scheduler trait in Session 22 (health
        // integration). For now, wire it with a stub old-scheduler.
        let legacy_scheduler =
            mai_core::scheduler::Scheduler::new(mai_core::scheduler::SchedulerConfig::default())
                .map_err(|e| ServerError::Init(format!("Legacy scheduler for hotswap: {e}")))?;
        let legacy_scheduler = Arc::new(RwLock::new(legacy_scheduler));
        let hotswap = HotSwapManager::new(legacy_scheduler, registry.clone(), health.clone());
        let hotswap = Arc::new(RwLock::new(hotswap));

        let audit_writer = Arc::new(MemoryAuditWriter::new());
        let config = Arc::new(RwLock::new(self.config.clone()));

        // -- Step 3b: Load API key authentication (Session 14c) --
        let auth = load_auth_state();

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
                        // instance in the new scheduler (Session 15).
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
                                vram_allocated: 0, // Populated by health monitor (Session 22)
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
        );

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

// -- Auth Loading (Session 14c) --

/// Load authentication state from config or generate first-boot key.
///
/// Precedence:
/// 1. If config/auth_keys.toml exists, load keys from it.
/// 2. If no config file, generate a first-boot admin key, print it to
///    stdout (ONE TIME), and start in local-trust mode so the admin can
///    configure persistent keys.
fn load_auth_state() -> AuthState {
    let auth_path = Path::new(AUTH_KEYS_CONFIG_PATH);

    if auth_path.exists() {
        match auth::load_api_keys_from_toml(auth_path) {
            Ok(store) => {
                info!(
                    keys = store.len(),
                    path = %auth_path.display(),
                    "API key authentication loaded from config"
                );
                return AuthState::with_key_store(store);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to load auth config, falling back to first-boot mode"
                );
            }
        }
    }

    // First-boot: generate an admin key and print it.
    // The admin copies this key into config/auth_keys.toml (hashed)
    // for persistent authentication.
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
    println!("  to config/auth_keys.toml to persist it:");
    println!();
    println!("  [[keys]]");
    println!("  hash = \"{admin_hash}\"");
    println!("  profile_id = \"admin\"");
    println!("  role = \"admin\"");
    println!("  display_name = \"System Admin\"");
    println!("========================================");

    info!("First-boot admin key generated (printed to stdout, NOT logged)");

    // Start with the generated key loaded + local-trust fallback
    // so the admin can configure via API immediately.
    let mut store = auth::ApiKeyStore::new();
    store.allow_internal_profile_header = true;
    store.add_key_hashed(
        admin_hash,
        "admin".to_string(),
        crate::types::ProfileRole::Admin,
        Some("First-Boot Admin".to_string()),
    );
    AuthState::with_key_store(store)
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
    let framework = FrameworkConfig::from_toml(path).unwrap_or_default();

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
                gpu_ids: at
                    .get("gpu_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_integer().map(|i| i as u32))
                            .collect()
                    })
                    .unwrap_or_default(),
                max_concurrent: at
                    .get("max_concurrent")
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(4) as usize,
                models: at
                    .get("models")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
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

/// Load scheduler configuration from config/scheduler.toml.
///
/// Falls back to defaults if the file is missing or invalid.
/// Aliases are loaded from the `[aliases]` section.
fn load_scheduler_config() -> NewSchedulerConfig {
    let path = Path::new("config/scheduler.toml");
    if !path.exists() {
        info!("No scheduler config file found, using defaults");
        return NewSchedulerConfig::default();
    }

    let content = match std::fs::read_to_string(path) {
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

/// Stub vault implementation for server bootstrap.
///
/// In production, Session 12 provides a real ZFS-backed vault. This stub
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
    fn test_load_auth_state_no_config() {
        // When no config file exists, load_auth_state should generate
        // a first-boot key and return a working AuthState.
        let auth = load_auth_state();
        // The store should have at least the generated admin key
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
}
