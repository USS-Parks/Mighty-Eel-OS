//! Hot-Swap Manager - Zero-downtime model/adapter replacement
//!
//! Coordinates graceful draining, component replacement, health verification,
//! and automatic rollback on failure. NEVER transmits data off-device.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::RwLock;

use crate::health::HealthMonitor;
use crate::registry::ModelRegistry;
use crate::scheduler::Scheduler;
use crate::types::{AdapterId, ModelId};

/// Swap target specification
#[derive(Debug, Clone)]
pub enum SwapTarget {
    Model {
        old_id: ModelId,
        new_id: ModelId,
    },
    Adapter {
        old_adapter: AdapterId,
        new_adapter: AdapterId,
    },
    Hardware {
        event: HardwareChangeEvent,
    },
}

#[derive(Debug, Clone)]
pub enum HardwareChangeEvent {
    GpuAdded { gpu_id: String },
    GpuRemoved { gpu_id: String },
    MemristorCardInserted,
    ThermalSensorAdded,
}

/// Swap request configuration
#[derive(Debug, Clone)]
pub struct SwapRequest {
    pub target: SwapTarget,
    /// Max time to finish in-flight requests
    pub drain_timeout: Duration,
    /// Max time for new component health check
    pub health_check_timeout: Duration,
    pub rollback_on_failure: bool,
}

/// Result of swap operation
// B2 FIX: Added Clone derive (all variants contain Clone-able types)
#[derive(Debug, Clone)]
pub enum SwapResult {
    Success {
        drained_requests: usize,
        completion_time: Duration,
    },
    RolledBack {
        reason: String,
        original_restored: bool,
    },
    Failed {
        error: SwapError,
        partial_state: SwapState,
    },
}

/// Intermediate state for audit/rollback
#[derive(Debug, Clone)]
pub struct SwapState {
    pub target: SwapTarget,
    pub drain_completed: bool,
    pub old_deactivated: bool,
    pub new_activated: bool,
    pub health_check_passed: Option<bool>,
}

/// Hot-swap manager errors
// B2 FIX: Added Clone derive (all variants are String/usize, all Clone-able)
#[derive(Error, Debug, Clone)]
pub enum SwapError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Drain timeout: {0} requests still in flight")]
    DrainTimeout(usize),

    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Rollback failed: {0}")]
    RollbackFailed(String),

    #[error("Scheduler coordination failed: {0}")]
    SchedulerError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),
}

/// Audit entry for swap operations
#[derive(Debug, Clone)]
pub struct SwapAuditEntry {
    pub timestamp: std::time::Instant,
    pub operator: Option<crate::types::ProfileId>,
    pub target: SwapTarget,
    pub reason: String,
    pub duration_ms: u64,
    pub result: SwapResult,
    pub requests_drained: usize,
    pub rollback_performed: bool,
}

/// Main hot-swap manager
pub struct HotSwapManager {
    scheduler: Arc<RwLock<Scheduler>>,
    registry: Arc<RwLock<ModelRegistry>>,
    health_monitor: Arc<RwLock<HealthMonitor>>,
    /// In production: append-only vault storage
    audit_log: Vec<SwapAuditEntry>,
}

impl HotSwapManager {
    /// Create new manager with core dependencies
    pub fn new(
        scheduler: Arc<RwLock<Scheduler>>,
        registry: Arc<RwLock<ModelRegistry>>,
        health_monitor: Arc<RwLock<HealthMonitor>>,
    ) -> Self {
        Self {
            scheduler,
            registry,
            health_monitor,
            audit_log: Vec::new(),
        }
    }

    /// Execute a swap request with graceful handling
    pub async fn execute_swap(&self, request: SwapRequest) -> Result<SwapResult, SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Pause routing to affected component
    async fn pause_routing(&self, target: &SwapTarget) -> Result<(), SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Drain in-flight requests with timeout
    async fn drain_requests(
        &self,
        target: &SwapTarget,
        timeout: Duration,
    ) -> Result<usize, SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Deactivate old component
    async fn deactivate(&self, target: &SwapTarget) -> Result<(), SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Activate new component
    async fn activate(&self, target: &SwapTarget) -> Result<(), SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Health check new component
    async fn health_check(
        &self,
        target: &SwapTarget,
        timeout: Duration,
    ) -> Result<bool, SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Rollback to previous state
    async fn rollback(&self, target: &SwapTarget) -> Result<(), SwapError> {
        // Implementation in Session 07
        todo!()
    }

    /// Record audit entry (append-only in production)
    fn record_audit(&mut self, entry: SwapAuditEntry) {
        self.audit_log.push(entry);
        // In production: also write to vault audit trail
    }

    /// Get recent swap history (for dashboard)
    pub fn recent_swaps(&self, limit: usize) -> &[SwapAuditEntry] {
        let start = self.audit_log.len().saturating_sub(limit);
        &self.audit_log[start..]
    }
}
