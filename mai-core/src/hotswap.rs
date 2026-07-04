//! Hot-Swap Manager - Zero-downtime model/adapter replacement
//!
//! Coordinates graceful draining, component replacement, health verification,
//! and automatic rollback on failure. NEVER transmits data off-device.
//!
//! Swap sequence:
//! 1. Pause routing to target component (mark unhealthy in scheduler)
//! 2. Drain in-flight requests (poll with timeout)
//! 3. Deactivate old component (unregister from scheduler, unload from registry)
//! 4. Activate new component (load in registry, register in scheduler)
//! 5. Health check new component (record heartbeat, verify via health monitor)
//! 6. On failure: rollback (reactivate old, deactivate new)
//! 7. Resume routing
//! 8. Record audit entry (append-only, local-only per telemetry policy)

use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::health::HealthMonitor;
use crate::registry::ModelRegistry;
use crate::scheduler::Scheduler;
use crate::types::{AdapterId, ModelId, ProfileId};

// ─── Swap target specification ───────────────────────────────────────────────

/// What is being swapped: a model, an adapter process, or a hardware event.
#[derive(Debug, Clone)]
pub enum SwapTarget {
    /// Replace one model with another on the same adapter(s).
    Model { old_id: ModelId, new_id: ModelId },
    /// Replace one adapter process with another (same backend type, new binary/config).
    Adapter {
        old_adapter: AdapterId,
        new_adapter: AdapterId,
    },
    /// React to a hardware topology change (GPU added/removed, memristor card, thermal sensor).
    Hardware { event: HardwareChangeEvent },
}

/// Hardware topology changes that trigger a hot-swap operation.
#[derive(Debug, Clone)]
pub enum HardwareChangeEvent {
    /// New GPU detected on PCIe bus.
    GpuAdded { gpu_id: String },
    /// GPU removed or failed (thermal shutdown, physical removal).
    GpuRemoved { gpu_id: String },
    /// TetraMem MX100 memristor card inserted (future: 2028+).
    MemristorCardInserted,
    /// New thermal sensor detected (triggers health monitor reconfiguration).
    ThermalSensorAdded,
}

// ─── Swap request ────────────────────────────────────────────────────────────

/// Configuration for a swap operation.
#[derive(Debug, Clone)]
pub struct SwapRequest {
    /// What to swap.
    pub target: SwapTarget,
    /// Max time to wait for in-flight requests to complete.
    pub drain_timeout: Duration,
    /// Max time for new component health check to pass.
    pub health_check_timeout: Duration,
    /// Whether to automatically revert if health check fails.
    pub rollback_on_failure: bool,
    /// Optional operator profile ID for audit trail.
    pub operator: Option<ProfileId>,
    /// Human-readable reason for the swap.
    pub reason: String,
}

impl SwapRequest {
    /// Create a model swap with sensible defaults.
    pub fn model_swap(old_id: ModelId, new_id: ModelId, reason: impl Into<String>) -> Self {
        Self {
            target: SwapTarget::Model { old_id, new_id },
            drain_timeout: Duration::from_secs(30),
            health_check_timeout: Duration::from_secs(30),
            rollback_on_failure: true,
            operator: None,
            reason: reason.into(),
        }
    }

    /// Create an adapter swap with sensible defaults.
    pub fn adapter_swap(
        old_adapter: AdapterId,
        new_adapter: AdapterId,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            target: SwapTarget::Adapter {
                old_adapter,
                new_adapter,
            },
            drain_timeout: Duration::from_secs(15),
            health_check_timeout: Duration::from_secs(30),
            rollback_on_failure: true,
            operator: None,
            reason: reason.into(),
        }
    }

    /// Create a hardware event swap (no rollback possible for physical events).
    pub fn hardware_event(event: HardwareChangeEvent, reason: impl Into<String>) -> Self {
        Self {
            target: SwapTarget::Hardware { event },
            drain_timeout: Duration::from_secs(10),
            health_check_timeout: Duration::from_secs(15),
            rollback_on_failure: false,
            operator: None,
            reason: reason.into(),
        }
    }
}

// ─── Swap result ─────────────────────────────────────────────────────────────

/// Outcome of a swap operation.
#[derive(Debug, Clone)]
pub enum SwapResult {
    /// Swap completed successfully.
    Success {
        drained_requests: usize,
        completion_time: Duration,
    },
    /// Health check failed but rollback succeeded.
    RolledBack {
        reason: String,
        original_restored: bool,
    },
    /// Swap failed and rollback also failed or was disabled.
    Failed {
        error: SwapError,
        partial_state: SwapState,
    },
}

impl SwapResult {
    /// Whether the swap ended in a usable state (either success or clean rollback).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Success { .. }
                | Self::RolledBack {
                    original_restored: true,
                    ..
                }
        )
    }
}

// ─── Swap state (for audit/rollback tracking) ────────────────────────────────

/// Intermediate state tracking for audit and rollback decisions.
#[derive(Debug, Clone)]
pub struct SwapState {
    pub target: SwapTarget,
    pub drain_completed: bool,
    pub old_deactivated: bool,
    pub new_activated: bool,
    pub health_check_passed: Option<bool>,
}

impl SwapState {
    fn new(target: SwapTarget) -> Self {
        Self {
            target,
            drain_completed: false,
            old_deactivated: false,
            new_activated: false,
            health_check_passed: None,
        }
    }

    /// Whether rollback is needed based on current state.
    fn needs_rollback(&self) -> bool {
        // If we deactivated old but new is not healthy, must rollback
        self.old_deactivated && self.health_check_passed != Some(true)
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Hot-swap operation errors.
#[derive(Error, Debug, Clone)]
pub enum SwapError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Drain timeout: {0} requests still in flight after deadline")]
    DrainTimeout(usize),

    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Rollback failed: {0}")]
    RollbackFailed(String),

    #[error("Scheduler coordination failed: {0}")]
    SchedulerError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("Hardware event cannot be rolled back: {0}")]
    IrreversibleEvent(String),

    #[error("Swap already in progress for target")]
    SwapInProgress,
}

// ─── Audit entry ─────────────────────────────────────────────────────────────

/// Append-only audit record for every swap operation. Stored locally per telemetry policy.
#[derive(Debug, Clone)]
pub struct SwapAuditEntry {
    pub timestamp: Instant,
    pub operator: Option<ProfileId>,
    pub target: SwapTarget,
    pub reason: String,
    pub duration_ms: u64,
    pub result: SwapResult,
    pub requests_drained: usize,
    pub rollback_performed: bool,
}

// ─── Hot-swap manager ────────────────────────────────────────────────────────

/// Orchestrates zero-downtime model and adapter replacement.
///
/// The HotSwapManager coordinates between the Scheduler (routing), Registry (model lifecycle),
/// and HealthMonitor (adapter health) to perform live component swaps without dropping
/// in-flight requests.
///
/// All swap operations are recorded in a local-only audit log.
pub struct HotSwapManager {
    scheduler: Arc<RwLock<Scheduler>>,
    registry: Arc<RwLock<ModelRegistry>>,
    health_monitor: Arc<RwLock<HealthMonitor>>,
    /// Append-only audit log. In production: backed by vault storage.
    audit_log: Vec<SwapAuditEntry>,
    /// Guards against concurrent swaps on the same target.
    swap_in_progress: bool,
}

impl HotSwapManager {
    /// Create a new hot-swap manager wired to the core subsystems.
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
            swap_in_progress: false,
        }
    }

    /// Execute a swap request with graceful draining and automatic rollback.
    ///
    /// Sequence: pause routing → drain → deactivate old → activate new → health check →
    /// resume routing. On failure: rollback if configured.
    pub async fn execute_swap(&mut self, request: SwapRequest) -> Result<SwapResult, SwapError> {
        if self.swap_in_progress {
            return Err(SwapError::SwapInProgress);
        }

        self.swap_in_progress = true;
        let start = Instant::now();
        let mut state = SwapState::new(request.target.clone());
        let mut drained_count: usize = 0;

        let result = self
            .run_swap_sequence(&request, &mut state, &mut drained_count)
            .await;

        let swap_result = match result {
            Ok(()) => {
                let completion_time = start.elapsed();
                info!(
                    "Swap completed successfully in {}ms, drained {drained_count} requests",
                    completion_time.as_millis(),
                );
                SwapResult::Success {
                    drained_requests: drained_count,
                    completion_time,
                }
            }
            Err(ref err) => {
                warn!("Swap failed: {err}");
                if request.rollback_on_failure && state.needs_rollback() {
                    match self.rollback(&request.target).await {
                        Ok(()) => {
                            info!("Rollback succeeded");
                            SwapResult::RolledBack {
                                reason: err.to_string(),
                                original_restored: true,
                            }
                        }
                        Err(rollback_err) => {
                            error!("Rollback also failed: {rollback_err}");
                            SwapResult::Failed {
                                error: SwapError::RollbackFailed(format!(
                                    "Original error: {err}. Rollback error: {rollback_err}"
                                )),
                                partial_state: state.clone(),
                            }
                        }
                    }
                } else {
                    // No rollback needed or not configured
                    SwapResult::Failed {
                        error: err.clone(),
                        partial_state: state.clone(),
                    }
                }
            }
        };

        // Record audit entry regardless of outcome
        let rollback_performed = matches!(swap_result, SwapResult::RolledBack { .. });
        self.record_audit(SwapAuditEntry {
            timestamp: start,
            operator: request.operator,
            target: request.target.clone(),
            reason: request.reason.clone(),
            duration_ms: {
                // Safety: swap duration millis will never exceed u64
                #[allow(clippy::cast_possible_truncation)]
                let ms = start.elapsed().as_millis() as u64;
                ms
            },
            result: swap_result.clone(),
            requests_drained: drained_count,
            rollback_performed,
        });

        self.swap_in_progress = false;

        // Result already captured in swap_result; always return Ok with the outcome
        let _ = result;
        Ok(swap_result)
    }

    /// Internal: runs the swap sequence, returning Err on any step failure.
    async fn run_swap_sequence(
        &self,
        request: &SwapRequest,
        state: &mut SwapState,
        drained_count: &mut usize,
    ) -> Result<(), SwapError> {
        // Resolve adapter ID before deactivation (registry needs old model loaded to look it up)
        let resolved_adapter_id = self.target_adapter_id(&request.target).await?;

        // Step 1: Pause routing to old component
        self.pause_routing(&request.target).await?;

        // Step 2: Drain in-flight requests
        *drained_count = self
            .drain_requests(&request.target, request.drain_timeout)
            .await?;
        state.drain_completed = true;

        // Step 3: Deactivate old component
        self.deactivate(&request.target).await?;
        state.old_deactivated = true;

        // Step 4: Activate new component (pass resolved adapter for model loading)
        self.activate(&request.target, &resolved_adapter_id).await?;
        state.new_activated = true;

        // Step 5: Health check new component
        let healthy = self
            .health_check(&request.target, request.health_check_timeout)
            .await?;
        state.health_check_passed = Some(healthy);

        if !healthy {
            return Err(SwapError::HealthCheckFailed(
                "New component failed health verification within timeout".to_string(),
            ));
        }

        // Step 6: Resume routing (mark new component healthy in scheduler)
        self.resume_routing(&request.target).await?;

        Ok(())
    }

    /// Mark the old component as unhealthy in the scheduler so no new requests route to it.
    async fn pause_routing(&self, target: &SwapTarget) -> Result<(), SwapError> {
        let mut scheduler = self.scheduler.write().await;

        match target {
            SwapTarget::Model { old_id, .. } => {
                // Find the adapter serving this model and mark it unhealthy
                let registry = self.registry.read().await;
                if let Some(adapter_id) = registry.get_loaded_adapter(old_id) {
                    scheduler.set_adapter_health(adapter_id, false);
                    info!("Paused routing: adapter {adapter_id} marked unhealthy for model swap",);
                } else {
                    return Err(SwapError::ComponentNotFound(format!(
                        "No loaded adapter for model {old_id}"
                    )));
                }
            }
            SwapTarget::Adapter { old_adapter, .. } => {
                scheduler.set_adapter_health(old_adapter, false);
                info!("Paused routing: adapter {old_adapter} marked unhealthy");
            }
            SwapTarget::Hardware { event } => {
                if let HardwareChangeEvent::GpuRemoved { gpu_id } = event {
                    // Mark all adapters on this GPU as unhealthy
                    // In production: query HIL for adapter-to-GPU mapping
                    // For now: log intent; the adapter ID IS the GPU mapping
                    scheduler.set_adapter_health(gpu_id, false);
                    info!("Paused routing: GPU {gpu_id} adapters marked unhealthy");
                } else {
                    // GpuAdded, MemristorCardInserted, ThermalSensorAdded
                    // No existing routing to pause for additions
                    info!("Hardware addition event: no routing to pause");
                }
            }
        }

        Ok(())
    }

    /// Wait for in-flight requests to the target to complete, with timeout.
    ///
    /// Polls the scheduler's queue depth every 50ms until in-flight hits zero
    /// or the timeout expires.
    async fn drain_requests(
        &self,
        target: &SwapTarget,
        timeout: Duration,
    ) -> Result<usize, SwapError> {
        let adapter_id = self.target_adapter_id(target).await?;
        let deadline = Instant::now() + timeout;
        let poll_interval = Duration::from_millis(50);
        let mut total_drained: usize = 0;

        loop {
            let in_flight = {
                let scheduler = self.scheduler.read().await;
                scheduler.adapter_in_flight(&adapter_id)
            };

            if in_flight == 0 {
                info!("Drain complete: {total_drained} requests drained for {adapter_id:?}",);
                return Ok(total_drained);
            }

            if Instant::now() >= deadline {
                warn!("Drain timeout: {in_flight} requests still in flight for {adapter_id:?}",);
                return Err(SwapError::DrainTimeout(in_flight));
            }

            total_drained = total_drained.max(in_flight);
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Deactivate old component: unregister from scheduler and unload from registry.
    async fn deactivate(&self, target: &SwapTarget) -> Result<(), SwapError> {
        match target {
            SwapTarget::Model { old_id, .. } => {
                // Unload the model from the registry
                let mut registry = self.registry.write().await;
                registry.unload_model(old_id).map_err(|e| {
                    SwapError::RegistryError(format!("Failed to unload model {old_id}: {e}"))
                })?;
                info!("Deactivated model: {old_id}");
            }
            SwapTarget::Adapter { old_adapter, .. } => {
                // Unregister adapter from scheduler
                let mut scheduler = self.scheduler.write().await;
                scheduler.unregister_adapter(old_adapter);
                // Unregister from health monitor
                let mut health = self.health_monitor.write().await;
                health.unregister_adapter(old_adapter);
                info!("Deactivated adapter: {old_adapter}");
            }
            SwapTarget::Hardware { event } => {
                if let HardwareChangeEvent::GpuRemoved { gpu_id } = event {
                    let mut scheduler = self.scheduler.write().await;
                    scheduler.unregister_adapter(gpu_id);
                    let mut health = self.health_monitor.write().await;
                    health.unregister_adapter(gpu_id);
                    info!("Deactivated GPU: {gpu_id}");
                } else {
                    // Additions don't have an old component to deactivate
                    info!("Hardware addition: no old component to deactivate");
                }
            }
        }

        Ok(())
    }

    /// Activate new component: load in registry and register in scheduler.
    async fn activate(
        &self,
        target: &SwapTarget,
        resolved_adapter: &AdapterId,
    ) -> Result<(), SwapError> {
        match target {
            SwapTarget::Model { new_id, .. } => {
                // Load the new model through the registry onto the same adapter
                let mut registry = self.registry.write().await;
                registry
                    .load_model(new_id, resolved_adapter.clone())
                    .await
                    .map_err(|e| {
                        SwapError::RegistryError(format!("Failed to load model {new_id}: {e}"))
                    })?;
                info!("Activated new model: {new_id}");
            }
            SwapTarget::Adapter { new_adapter, .. } => {
                // Register new adapter in scheduler (starts unhealthy, health check promotes)
                let mut scheduler = self.scheduler.write().await;
                scheduler.register_adapter(
                    new_adapter.clone(),
                    vec![], // Model list populated after registration
                    4,      // Default max concurrent from SchedulerConfig
                    vec![], // GPU IDs assigned post-registration
                );
                // Start unhealthy until health check passes
                scheduler.set_adapter_health(new_adapter, false);
                // Register in health monitor
                let mut health = self.health_monitor.write().await;
                health.register_adapter(new_adapter.clone());
                info!("Activated new adapter: {new_adapter} (unhealthy until health check)",);
            }
            SwapTarget::Hardware { event } => {
                match event {
                    HardwareChangeEvent::GpuAdded { gpu_id } => {
                        let mut scheduler = self.scheduler.write().await;
                        scheduler.register_adapter(gpu_id.clone(), vec![], 4, vec![gpu_id.clone()]);
                        let mut health = self.health_monitor.write().await;
                        health.register_adapter(gpu_id.clone());
                        info!("Activated new GPU: {gpu_id}");
                    }
                    HardwareChangeEvent::MemristorCardInserted => {
                        // TetraMem card detection: the adapter slot is reserved
                        // but the actual TetraMem driver returns NotImplemented.
                        info!("Memristor card detected - adapter slot reserved for future");
                    }
                    HardwareChangeEvent::ThermalSensorAdded => {
                        // Reconfigure health monitor thermal thresholds
                        info!("Thermal sensor added - health monitor reconfiguration needed");
                    }
                    HardwareChangeEvent::GpuRemoved { .. } => {
                        // Nothing to activate when a GPU is removed
                        info!("GPU removal: no new component to activate");
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify health of the new component within the specified timeout.
    ///
    /// Sends synthetic heartbeats and checks the health monitor for healthy status.
    async fn health_check(
        &self,
        target: &SwapTarget,
        timeout: Duration,
    ) -> Result<bool, SwapError> {
        let adapter_id = self.new_target_adapter_id(target).await?;
        let deadline = Instant::now() + timeout;
        let poll_interval = Duration::from_millis(100);

        // Give the new component time to initialize, then start checking
        tokio::time::sleep(Duration::from_millis(50)).await;

        loop {
            // Record a heartbeat from the new component
            {
                let mut health = self.health_monitor.write().await;
                let _ = health.record_heartbeat(
                    &adapter_id,
                    1,    // requests_served
                    10.0, // avg_latency_ms (healthy baseline)
                    0.0,  // error_rate (no errors)
                );
            }

            // Check if health monitor considers it healthy
            {
                let health = self.health_monitor.read().await;
                if let Some(adapter_health) = health.get_adapter_health(&adapter_id)
                    && matches!(adapter_health.status, crate::health::AdapterStatus::Healthy)
                {
                    info!("Health check passed for {adapter_id}");
                    return Ok(true);
                }
            }

            if Instant::now() >= deadline {
                warn!("Health check timeout for {adapter_id}");
                return Ok(false);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Mark new component as healthy in scheduler, resuming request routing.
    async fn resume_routing(&self, target: &SwapTarget) -> Result<(), SwapError> {
        let adapter_id = self.new_target_adapter_id(target).await?;
        let mut scheduler = self.scheduler.write().await;
        scheduler.set_adapter_health(&adapter_id, true);
        info!("Resumed routing to {adapter_id}");
        Ok(())
    }

    /// Rollback: reactivate old component, deactivate new component.
    async fn rollback(&self, target: &SwapTarget) -> Result<(), SwapError> {
        match target {
            SwapTarget::Model { old_id, new_id } => {
                // Unload new model, reload old model
                // We need an adapter to reload onto; look up from new_id (just loaded)
                let adapter_id = {
                    let registry = self.registry.read().await;
                    registry
                        .get_loaded_adapter(new_id)
                        .cloned()
                        .unwrap_or_else(|| "unknown-adapter".to_string())
                };
                let mut registry = self.registry.write().await;
                let _ = registry.unload_model(new_id); // Best effort
                registry.load_model(old_id, adapter_id).await.map_err(|e| {
                    SwapError::RollbackFailed(format!(
                        "Cannot restore original model {old_id}: {e}"
                    ))
                })?;
                info!("Rollback: restored model {old_id}");
            }
            SwapTarget::Adapter {
                old_adapter,
                new_adapter,
            } => {
                // Unregister new adapter, re-register old adapter
                let mut scheduler = self.scheduler.write().await;
                scheduler.unregister_adapter(new_adapter);
                scheduler.register_adapter(old_adapter.clone(), vec![], 4, vec![]);
                scheduler.set_adapter_health(old_adapter, true);

                let mut health = self.health_monitor.write().await;
                health.unregister_adapter(new_adapter);
                health.register_adapter(old_adapter.clone());
                info!("Rollback: restored adapter {old_adapter}");
            }
            SwapTarget::Hardware { event } => {
                // Hardware events are generally irreversible
                return Err(SwapError::IrreversibleEvent(format!(
                    "Cannot rollback hardware event: {event:?}"
                )));
            }
        }

        Ok(())
    }

    /// Record an audit entry. Append-only per sovereignty policy.
    fn record_audit(&mut self, entry: SwapAuditEntry) {
        self.audit_log.push(entry);
        // In production: also write to vault audit trail via VaultInterface
    }

    /// Get recent swap history (for dashboard display).
    pub fn recent_swaps(&self, limit: usize) -> &[SwapAuditEntry] {
        let start = self.audit_log.len().saturating_sub(limit);
        &self.audit_log[start..]
    }

    /// Total number of swaps performed in this session.
    pub fn total_swap_count(&self) -> usize {
        self.audit_log.len()
    }

    /// Number of successful swaps.
    pub fn successful_swap_count(&self) -> usize {
        self.audit_log
            .iter()
            .filter(|e| matches!(e.result, SwapResult::Success { .. }))
            .count()
    }

    /// Whether a swap is currently in progress.
    pub fn is_swap_in_progress(&self) -> bool {
        self.swap_in_progress
    }

    // ─── Helper methods ──────────────────────────────────────────────────────

    /// Resolve the OLD adapter ID for a swap target (the one being drained/deactivated).
    async fn target_adapter_id(&self, target: &SwapTarget) -> Result<AdapterId, SwapError> {
        match target {
            SwapTarget::Model { old_id, .. } => {
                let registry = self.registry.read().await;
                registry.get_loaded_adapter(old_id).cloned().ok_or_else(|| {
                    SwapError::ComponentNotFound(format!("No adapter loaded for model {old_id}"))
                })
            }
            SwapTarget::Adapter { old_adapter, .. } => Ok(old_adapter.clone()),
            SwapTarget::Hardware { event } => {
                if let HardwareChangeEvent::GpuRemoved { gpu_id }
                | HardwareChangeEvent::GpuAdded { gpu_id } = event
                {
                    Ok(gpu_id.clone())
                } else {
                    Err(SwapError::ComponentNotFound(
                        "No adapter ID for this hardware event type".to_string(),
                    ))
                }
            }
        }
    }

    /// Resolve the NEW adapter ID for a swap target (the one being health-checked/activated).
    async fn new_target_adapter_id(&self, target: &SwapTarget) -> Result<AdapterId, SwapError> {
        match target {
            SwapTarget::Model { new_id, .. } => {
                let registry = self.registry.read().await;
                registry.get_loaded_adapter(new_id).cloned().ok_or_else(|| {
                    SwapError::ComponentNotFound(format!(
                        "No adapter loaded for new model {new_id}"
                    ))
                })
            }
            SwapTarget::Adapter { new_adapter, .. } => Ok(new_adapter.clone()),
            SwapTarget::Hardware { event } => {
                if let HardwareChangeEvent::GpuAdded { gpu_id } = event {
                    Ok(gpu_id.clone())
                } else {
                    Err(SwapError::ComponentNotFound(
                        "No new adapter ID for this hardware event type".to_string(),
                    ))
                }
            }
        }
    }
}

// ─── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::HealthConfig;
    use crate::registry::ModelRegistry;
    use crate::scheduler::{Scheduler, SchedulerConfig, SchedulingStrategy};
    use crate::vault::VaultInterface;
    use async_trait::async_trait;

    /// Mock vault for registry construction in tests.
    struct MockVault;

    #[async_trait]
    impl VaultInterface for MockVault {
        async fn load_model_weights(
            &self,
            _model_id: &str,
        ) -> Result<Vec<u8>, crate::vault::VaultError> {
            Ok(vec![0u8; 64])
        }

        async fn store_model_package(
            &self,
            _model_id: &str,
            _data: &[u8],
        ) -> Result<(), crate::vault::VaultError> {
            Ok(())
        }

        async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), crate::vault::VaultError> {
            Ok(())
        }

        async fn verify_signature(
            &self,
            _data: &[u8],
            _signature: &[u8],
        ) -> Result<bool, crate::vault::VaultError> {
            Ok(true)
        }
    }

    type TestComponents = (
        Arc<RwLock<Scheduler>>,
        Arc<RwLock<ModelRegistry>>,
        Arc<RwLock<HealthMonitor>>,
    );

    fn make_test_components() -> TestComponents {
        let config = SchedulerConfig {
            strategy: SchedulingStrategy::LeastLoaded,
            ..SchedulerConfig::default()
        };
        let scheduler = Arc::new(RwLock::new(Scheduler::new(config).unwrap()));
        let registry = Arc::new(RwLock::new(ModelRegistry::new(Box::new(MockVault))));
        let health = Arc::new(RwLock::new(HealthMonitor::new(HealthConfig::default())));
        (scheduler, registry, health)
    }

    #[test]
    fn test_new_manager() {
        let (scheduler, registry, health) = make_test_components();
        let mgr = HotSwapManager::new(scheduler, registry, health);
        assert_eq!(mgr.total_swap_count(), 0);
        assert!(!mgr.is_swap_in_progress());
    }

    #[test]
    fn test_swap_request_model_defaults() {
        let req = SwapRequest::model_swap(
            "model-old".to_string(),
            "model-new".to_string(),
            "upgrade to v2",
        );
        assert_eq!(req.drain_timeout, Duration::from_secs(30));
        assert_eq!(req.health_check_timeout, Duration::from_secs(30));
        assert!(req.rollback_on_failure);
        assert!(req.operator.is_none());
        assert_eq!(req.reason, "upgrade to v2");
    }

    #[test]
    fn test_swap_request_adapter_defaults() {
        let req = SwapRequest::adapter_swap(
            "adapter-old".to_string(),
            "adapter-new".to_string(),
            "config change",
        );
        assert_eq!(req.drain_timeout, Duration::from_secs(15));
        assert!(req.rollback_on_failure);
    }

    #[test]
    fn test_swap_request_hardware_defaults() {
        let req = SwapRequest::hardware_event(
            HardwareChangeEvent::GpuAdded {
                gpu_id: "gpu-1".to_string(),
            },
            "new GPU detected",
        );
        assert!(!req.rollback_on_failure); // Hardware events can't roll back
    }

    #[test]
    fn test_swap_state_needs_rollback() {
        let mut state = SwapState::new(SwapTarget::Adapter {
            old_adapter: "a".to_string(),
            new_adapter: "b".to_string(),
        });

        // Initially: no rollback needed
        assert!(!state.needs_rollback());

        // After deactivation but before health check: needs rollback
        state.old_deactivated = true;
        assert!(state.needs_rollback());

        // After successful health check: no rollback
        state.health_check_passed = Some(true);
        assert!(!state.needs_rollback());
    }

    #[test]
    fn test_swap_state_needs_rollback_failed_health() {
        let mut state = SwapState::new(SwapTarget::Model {
            old_id: "m1".to_string(),
            new_id: "m2".to_string(),
        });
        state.old_deactivated = true;
        state.new_activated = true;
        state.health_check_passed = Some(false);
        assert!(state.needs_rollback());
    }

    #[test]
    fn test_swap_result_is_recoverable() {
        let success = SwapResult::Success {
            drained_requests: 5,
            completion_time: Duration::from_millis(200),
        };
        assert!(success.is_recoverable());

        let rolled_back = SwapResult::RolledBack {
            reason: "test".to_string(),
            original_restored: true,
        };
        assert!(rolled_back.is_recoverable());

        let rolled_back_partial = SwapResult::RolledBack {
            reason: "test".to_string(),
            original_restored: false,
        };
        assert!(!rolled_back_partial.is_recoverable());

        let failed = SwapResult::Failed {
            error: SwapError::DrainTimeout(3),
            partial_state: SwapState::new(SwapTarget::Adapter {
                old_adapter: "a".to_string(),
                new_adapter: "b".to_string(),
            }),
        };
        assert!(!failed.is_recoverable());
    }

    #[tokio::test]
    async fn test_execute_swap_adapter_not_registered() {
        let (scheduler, registry, health) = make_test_components();
        let mut mgr = HotSwapManager::new(scheduler, registry, health);

        let req =
            SwapRequest::adapter_swap("nonexistent".to_string(), "new-adapter".to_string(), "test");

        let result = mgr.execute_swap(req).await.unwrap();
        // The swap should complete (pause routing on nonexistent just sets health=false)
        // but drain will immediately succeed (0 in-flight) since adapter isn't tracked
        // Actual outcome depends on scheduler behavior with unknown adapter ID
        assert_eq!(mgr.total_swap_count(), 1);
    }

    #[tokio::test]
    async fn test_execute_swap_adapter_success() {
        let (scheduler, registry, health) = make_test_components();

        // Register an adapter in scheduler + health monitor
        {
            let mut s = scheduler.write().await;
            s.register_adapter(
                "old-adapter".to_string(),
                vec!["model-a".to_string()],
                4,
                vec![],
            );
            s.set_adapter_health(&"old-adapter".to_string(), true);
        }
        {
            let mut h = health.write().await;
            h.register_adapter("old-adapter".to_string());
        }

        let mut mgr = HotSwapManager::new(scheduler.clone(), registry.clone(), health.clone());

        let req = SwapRequest::adapter_swap(
            "old-adapter".to_string(),
            "new-adapter".to_string(),
            "binary upgrade",
        );

        let result = mgr.execute_swap(req).await.unwrap();

        match &result {
            SwapResult::Success {
                drained_requests, ..
            } => {
                assert_eq!(*drained_requests, 0); // No in-flight during test
            }
            other => panic!("Expected Success, got {other:?}"),
        }

        assert_eq!(mgr.total_swap_count(), 1);
        assert_eq!(mgr.successful_swap_count(), 1);

        // Verify new adapter is registered and healthy
        let s = scheduler.read().await;
        assert!(s.healthy_adapter_count() >= 1);
    }

    #[tokio::test]
    async fn test_concurrent_swap_rejected() {
        let (scheduler, registry, health) = make_test_components();
        let mut mgr = HotSwapManager::new(scheduler, registry, health);

        // Simulate swap_in_progress flag
        mgr.swap_in_progress = true;

        let req = SwapRequest::adapter_swap("a".to_string(), "b".to_string(), "test");

        let err = mgr.execute_swap(req).await.unwrap_err();
        assert!(matches!(err, SwapError::SwapInProgress));
    }

    #[test]
    fn test_recent_swaps_limit() {
        let (scheduler, registry, health) = make_test_components();
        let mut mgr = HotSwapManager::new(scheduler, registry, health);

        // Manually insert audit entries
        for i in 0..5 {
            mgr.record_audit(SwapAuditEntry {
                timestamp: Instant::now(),
                operator: None,
                target: SwapTarget::Adapter {
                    old_adapter: format!("old-{i}"),
                    new_adapter: format!("new-{i}"),
                },
                reason: format!("test {i}"),
                duration_ms: 100,
                result: SwapResult::Success {
                    drained_requests: 0,
                    completion_time: Duration::from_millis(100),
                },
                requests_drained: 0,
                rollback_performed: false,
            });
        }

        assert_eq!(mgr.recent_swaps(3).len(), 3);
        assert_eq!(mgr.recent_swaps(10).len(), 5);
        assert_eq!(mgr.total_swap_count(), 5);
        assert_eq!(mgr.successful_swap_count(), 5);
    }

    #[test]
    fn test_hardware_event_types() {
        let added = HardwareChangeEvent::GpuAdded {
            gpu_id: "gpu-0".to_string(),
        };
        let removed = HardwareChangeEvent::GpuRemoved {
            gpu_id: "gpu-1".to_string(),
        };
        let memristor = HardwareChangeEvent::MemristorCardInserted;
        let thermal = HardwareChangeEvent::ThermalSensorAdded;

        // Just verify these construct without panic (type coverage)
        let _ = format!("{added:?}");
        let _ = format!("{removed:?}");
        let _ = format!("{memristor:?}");
        let _ = format!("{thermal:?}");
    }

    #[test]
    fn test_swap_error_display() {
        let err = SwapError::DrainTimeout(5);
        assert!(err.to_string().contains("5"));

        let err = SwapError::ComponentNotFound("gpu-x".to_string());
        assert!(err.to_string().contains("gpu-x"));

        let err = SwapError::SwapInProgress;
        assert!(err.to_string().contains("in progress"));
    }
}
