//! Application state shared across all axum handlers.
//!
//! AppState holds Arc references to every mai-core component the API
//! server needs. It is injected into handlers via axum's State extractor.
//! All components are thread-safe (Arc + Mutex/RwLock internally).

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::audit::AuditWriter;
use crate::auth::AuthState;
use crate::config::ServerConfig;
use crate::metrics::MetricsRegistry;
use crate::openbao_client::OpenBaoBridgeClient;
use crate::production_guard::RuntimeChecks;
use crate::rate_limit::RateLimiter;
use crate::ship_profile::ShipProfile;
use crate::trust_builder::TrustExchangeMode;

use mai_adapters::manager::AdapterManager;
use mai_compliance::audit::AuditLog as ComplianceAuditLog;
use mai_compliance::bundle::{AcceptAllBundleVerifier, BundleVerifier};
use mai_compliance::policy::{PolicyManager, PolicyTemplate};
use mai_compliance::reports::ReportManager;
use mai_compliance::trust_cache::{CacheThresholds, LocalTrustCache};
use mai_core::airgap::AirGapPolicy;
use mai_core::health::HealthMonitor;
use mai_core::hotswap::HotSwapManager;
use mai_core::power::PowerStateMachine;
use mai_core::registry::ModelRegistry;
use mai_scheduler::Scheduler;
use mai_scheduler::metrics::MetricsCollector;

/// Shared application state for all request handlers.
///
/// Cloned into each handler via `axum::extract::State<AppState>`.
/// All inner fields are behind Arc so cloning is cheap (pointer bump).
#[derive(Clone)]
pub struct AppState {
    /// Model scheduler: routes inference requests to instances (mai-scheduler)
    pub scheduler: Arc<dyn Scheduler>,
    /// Model registry: manifest management and lifecycle tracking
    pub registry: Arc<RwLock<ModelRegistry>>,
    /// Health monitor: adapter heartbeats, hardware telemetry, alerts
    pub health: Arc<RwLock<HealthMonitor>>,
    /// Power state machine: sleep mode transitions
    pub power: Arc<RwLock<PowerStateMachine>>,
    /// Hot-swap manager: zero-downtime model updates
    pub hotswap: Arc<RwLock<HotSwapManager>>,
    /// Audit trail writer (trait object for testability)
    pub audit_writer: Arc<dyn AuditWriter>,
    /// Server configuration (may be hot-reloaded)
    pub config: Arc<RwLock<ServerConfig>>,
    /// Authentication state (token validator)
    pub auth: AuthState,
    /// Adapter manager: spawns and manages Python adapter subprocesses
    pub adapter_manager: Arc<Mutex<AdapterManager>>,
    /// Metrics collector: request lifecycle, health scoring, anomaly detection
    pub metrics_collector: Arc<MetricsCollector>,
    /// canonical connectivity state shared with mai-adapters
    /// and mai-compliance. Defaults to `AirGapped` when constructed via
    /// [`AppState::new`]; override with [`AppState::with_airgap_policy`].
    pub airgap_policy: AirGapPolicy,
    /// local trust cache. Holds the most recent signed policy
    /// bundle plus per-claim revocation snapshots. Defaults to an empty
    /// cache with stock thresholds; production wires a real refresher.
    pub trust_cache: Arc<RwLock<LocalTrustCache>>,
    /// verifier used when ingesting a signed bundle. Defaults to
    /// [`AcceptAllBundleVerifier`] so bring-up works without a key
    /// material; production wires `MlDsaBundleVerifier` with the
    /// vault-anchored registry.
    pub bundle_verifier: Arc<dyn BundleVerifier + Send + Sync>,
    /// Policy runtime (composer + decision cache + audit feed).
    /// Internally `Arc<Mutex<…>>` so cloning the AppState is cheap.
    pub policy_manager: PolicyManager,
    /// Tamper-evident compliance audit log.
    pub compliance_audit: ComplianceAuditLog,
    /// Compliance report generator façade.
    pub report_manager: Arc<ReportManager>,
    /// SHIP-07 Slice B: selected `POST /v1/auth/exchange_token` mode.
    /// Defaults to [`TrustExchangeMode::LocalDevSynthetic`] so the
    /// no-profile bring-up path keeps minting the synthetic local-dev
    /// token. Production startup swaps this to
    /// [`TrustExchangeMode::OpenBaoBridge`]; profiles that opt out
    /// entirely set it to [`TrustExchangeMode::Disabled`].
    pub trust_exchange_mode: TrustExchangeMode,
    /// SHIP-07 Slice B: snapshot of the ship profile + runtime
    /// introspection that drove `MaiServer::run()`. `None` when the
    /// server booted without a ship profile (legacy/test path); `Some`
    /// when one was loaded. Drives `GET /v1/system/production-readiness`.
    pub ship_readiness: Option<ShipReadiness>,
    /// SHIP-11: Prometheus-compatible metrics registry. Populated by
    /// [`crate::middleware::metrics_middleware`] for every request and
    /// rendered at `GET /v1/metrics`. Pre-seeded with the SHIP-11
    /// metric families so `# TYPE` lines appear in the exposition
    /// even before the first observation.
    pub metrics_registry: Arc<MetricsRegistry>,
    /// SEC-95 (closes SEC-011-MAI): optional token-bucket rate limiter.
    /// `None` means rate limiting is disabled — the middleware passes
    /// every request through, matching pre-SEC-95 behavior. Production
    /// startup constructs a [`RateLimiter`] keyed by route prefix and
    /// installs it via [`AppState::with_rate_limiter`]. See
    /// [`crate::rate_limit`] for the bucket implementation and
    /// [`crate::middleware::rate_limit_middleware`] for the axum glue.
    pub rate_limiter: Option<Arc<RateLimiter>>,
    /// OpenBao bridge client for claim issuance and trust cache refresh.
    /// `None` when the server booted without a ship profile or when
    /// `TrustExchangeMode` is not `OpenBaoBridge`. Wrapped in
    /// `Arc<RwLock<...>>` so credential rotation (TLM-4) can hot-swap
    /// the client without a process restart.
    pub openbao_bridge: Arc<RwLock<Option<OpenBaoBridgeClient>>>,
    /// Consecutive OpenBao connectivity failures since the last
    /// successful probe. Reset to 0 on success, incremented on each
    /// error. Drives the trust status endpoint's health assessment.
    pub openbao_consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
    /// Cancellation token signalled when the server is shutting down.
    /// The background trust-refresh loop checks this on each iteration
    /// to exit cleanly.
    pub cancel_token: CancellationToken,
}

/// SHIP-07 Slice B: captured ship-profile + runtime introspection.
///
/// `MaiServer::run()` installs this onto [`AppState`] after the
/// SHIP-03/04/05/06 builders have run. The readiness endpoint
/// recomputes a fresh [`crate::production_guard::ProductionReadinessReport`]
/// from these two fields on every request, so operators always see the
/// latest evaluation against the runtime that actually booted.
#[derive(Clone)]
pub struct ShipReadiness {
    /// The parsed ship profile the server booted with.
    pub profile: Arc<ShipProfile>,
    /// Runtime introspection collected during `apply_ship_profile`.
    pub runtime_checks: Arc<RuntimeChecks>,
}

impl AppState {
    /// Construct a new AppState from pre-built components.
    ///
    /// All components must be fully initialized before constructing AppState.
    /// The API server does not own component lifecycle; it borrows via Arc.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scheduler: Arc<dyn Scheduler>,
        registry: Arc<RwLock<ModelRegistry>>,
        health: Arc<RwLock<HealthMonitor>>,
        power: Arc<RwLock<PowerStateMachine>>,
        hotswap: Arc<RwLock<HotSwapManager>>,
        audit_writer: Arc<dyn AuditWriter>,
        config: Arc<RwLock<ServerConfig>>,
        auth: AuthState,
        adapter_manager: Arc<Mutex<AdapterManager>>,
        metrics_collector: Arc<MetricsCollector>,
    ) -> Self {
        let compliance_audit = ComplianceAuditLog::builder().build();
        let report_manager = Arc::new(ReportManager::builder(compliance_audit.clone()).build());
        let trust_cache = LocalTrustCache::new(CacheThresholds::default())
            .expect("default trust-cache thresholds are valid");
        Self {
            scheduler,
            registry,
            health,
            power,
            hotswap,
            audit_writer,
            config,
            auth,
            adapter_manager,
            metrics_collector,
            airgap_policy: AirGapPolicy::default(),
            trust_cache: Arc::new(RwLock::new(trust_cache)),
            bundle_verifier: Arc::new(AcceptAllBundleVerifier),
            policy_manager: PolicyManager::from_template(PolicyTemplate::Standard),
            compliance_audit,
            report_manager,
            trust_exchange_mode: TrustExchangeMode::LocalDevSynthetic,
            ship_readiness: None,
            // SHIP-11: pre-seed every metric family so dashboards see a
            // complete `# TYPE` line set on the first scrape, even
            // before the first request is handled.
            metrics_registry: Arc::new(MetricsRegistry::with_ship_11_defaults()),
            // SEC-95: disabled by default so existing tests and the
            // no-profile bring-up path are unchanged. Production wires
            // a limiter via [`AppState::with_rate_limiter`].
            rate_limiter: None,
            openbao_bridge: Arc::new(RwLock::new(None)),
            openbao_consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            cancel_token: CancellationToken::new(),
        }
    }

    /// Replace the air-gap policy in this state. Used by server bootstrap
    /// to inject a policy that's already wired to the hardware switch
    /// reader or to a deterministic dev-mode policy.
    #[must_use]
    pub fn with_airgap_policy(mut self, policy: AirGapPolicy) -> Self {
        self.airgap_policy = policy;
        self
    }

    /// Override the local trust cache. Used at bootstrap to inject a
    /// cache that's already pre-loaded from disk or wired to a
    /// background refresher.
    #[must_use]
    pub fn with_trust_cache(mut self, cache: Arc<RwLock<LocalTrustCache>>) -> Self {
        self.trust_cache = cache;
        self
    }

    /// Override the bundle verifier. Production wires
    /// `MlDsaBundleVerifier` with the vault-anchored registry; tests
    /// keep the [`AcceptAllBundleVerifier`] default.
    #[must_use]
    pub fn with_bundle_verifier(mut self, verifier: Arc<dyn BundleVerifier + Send + Sync>) -> Self {
        self.bundle_verifier = verifier;
        self
    }

    /// Override the policy manager. Bootstrap may wire a manager
    /// pre-loaded from a tenant-specific template (Healthcare /
    /// Defense / TribalGovernment).
    #[must_use]
    pub fn with_policy_manager(mut self, manager: PolicyManager) -> Self {
        self.policy_manager = manager;
        self
    }

    /// Override the compliance audit log and rebuild the dependent
    /// [`ReportManager`] so the report engine queries the new log.
    #[must_use]
    pub fn with_compliance_audit(mut self, audit: ComplianceAuditLog) -> Self {
        let report_manager = Arc::new(ReportManager::builder(audit.clone()).build());
        self.compliance_audit = audit;
        self.report_manager = report_manager;
        self
    }

    /// Override the report manager directly (e.g. when a custom
    /// template registry has been registered).
    #[must_use]
    pub fn with_report_manager(mut self, manager: Arc<ReportManager>) -> Self {
        self.report_manager = manager;
        self
    }

    /// SHIP-07 Slice B: install the selected token-exchange mode. The
    /// `POST /v1/auth/exchange_token` handler switches on this value
    /// instead of unconditionally minting a synthetic local-dev token.
    #[must_use]
    pub fn with_trust_exchange_mode(mut self, mode: TrustExchangeMode) -> Self {
        self.trust_exchange_mode = mode;
        self
    }

    /// Attach the OpenBao bridge client for claim issuance and trust
    /// cache refresh. Production startup calls this when
    /// [`TrustExchangeMode::OpenBaoBridge`] is selected.
    #[must_use]
    pub fn with_openbao_bridge(self, bridge: OpenBaoBridgeClient) -> Self {
        // Blocking write on construction path — server boot is
        // single-threaded at this point, no contention.
        *self.openbao_bridge.blocking_write() = Some(bridge);
        self
    }

    /// SHIP-07 Slice B: install the captured ship-profile + runtime
    /// introspection so `GET /v1/system/production-readiness` can
    /// recompute the report on demand.
    #[must_use]
    pub fn with_ship_readiness(mut self, readiness: ShipReadiness) -> Self {
        self.ship_readiness = Some(readiness);
        self
    }

    /// SHIP-11: override the metrics registry. Tests use this to inject
    /// a freshly-defaulted (empty) registry so assertions about
    /// `requests_total` start from zero instead of the pre-seeded
    /// SHIP-11 defaults.
    #[must_use]
    pub fn with_metrics_registry(mut self, registry: Arc<MetricsRegistry>) -> Self {
        self.metrics_registry = registry;
        self
    }

    /// SEC-95: install a token-bucket rate limiter. Production startup
    /// builds a [`RateLimiter`] with the per-route prefix configuration
    /// from `ServerConfig` and calls this. Tests that exercise the
    /// 429 path inject a tight-capacity limiter here.
    #[must_use]
    pub fn with_rate_limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time check: AppState must be Clone + Send + Sync
    fn _assert_clone_send_sync<T: Clone + Send + Sync>() {}

    #[test]
    fn test_appstate_is_clone_send_sync() {
        _assert_clone_send_sync::<AppState>();
    }
}
