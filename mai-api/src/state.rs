//! Application state shared across all axum handlers.
//!
//! AppState holds Arc references to every mai-core component the API
//! server needs. It is injected into handlers via axum's State extractor.
//! All components are thread-safe (Arc + Mutex/RwLock internally).

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::audit::AuditWriter;
use crate::auth::AuthState;
use crate::config::ServerConfig;

use mai_adapters::manager::AdapterManager;
use mai_core::health::HealthMonitor;
use mai_core::hotswap::HotSwapManager;
use mai_core::power::PowerStateMachine;
use mai_core::registry::ModelRegistry;
use mai_scheduler::metrics::MetricsCollector;
use mai_scheduler::Scheduler;

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
        }
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
