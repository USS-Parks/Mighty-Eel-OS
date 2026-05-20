//! Application state shared across all axum handlers.
//!
//! AppState holds Arc references to every mai-core component the API
//! server needs. It is injected into handlers via axum's State extractor.
//! All components are thread-safe (Arc + Mutex/RwLock internally).

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::audit::AuditWriter;
use crate::auth::AuthState;
use crate::config::ServerConfig;

use mai_core::health::HealthMonitor;
use mai_core::hotswap::HotSwapManager;
use mai_core::power::PowerStateMachine;
use mai_core::registry::ModelRegistry;
use mai_core::scheduler::Scheduler;

/// Shared application state for all request handlers.
///
/// Cloned into each handler via `axum::extract::State<AppState>`.
/// All inner fields are behind Arc so cloning is cheap (pointer bump).
#[derive(Clone)]
pub struct AppState {
    /// Model scheduler: routes inference requests to adapters
    pub scheduler: Arc<RwLock<Scheduler>>,
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
}

impl AppState {
    /// Construct a new AppState from pre-built components.
    ///
    /// All components must be fully initialized before constructing AppState.
    /// The API server does not own component lifecycle; it borrows via Arc.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scheduler: Arc<RwLock<Scheduler>>,
        registry: Arc<RwLock<ModelRegistry>>,
        health: Arc<RwLock<HealthMonitor>>,
        power: Arc<RwLock<PowerStateMachine>>,
        hotswap: Arc<RwLock<HotSwapManager>>,
        audit_writer: Arc<dyn AuditWriter>,
        config: Arc<RwLock<ServerConfig>>,
        auth: AuthState,
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
