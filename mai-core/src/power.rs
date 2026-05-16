//! Power State Machine - Sleep mode management and transitions
//!
//! Manages transitions between DeepVaultSleep, Sentinel, FullInference,
//! and ThermalThrottle states with hardware integration via HIL.
//! NEVER transmits data off-device.

use std::time::Duration;
use std::sync::Arc;

use thiserror::Error;

use mai_hil::PowerStateController;
use crate::types::TransitionId;
use crate::scheduler::ComplexityScore;

/// Power states (matches Tock-inspired trust model)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Off,
    DeepVaultSleep,  // ~2W GPU-era, ~1W QM-era target
    Sentinel,        // ~8W GPU-era, ~3W QM-era target
    FullInference,   // ~350W GPU-era, ~15W QM-era target
    ThermalThrottle,
}

/// Triggers for state transitions
#[derive(Debug, Clone)]
pub enum TransitionTrigger {
    SystemBoot,
    WakeTrigger { source: WakeSource },
    UrgentWake { source: WakeSource },
    RequestComplexity(ComplexityScore),
    AutoDemotion,
    ExtendedIdle,
    ThermalEvent { temperature_celsius: f64 },
    ThermalRecovered,
    ShutdownCommand,
}

#[derive(Debug, Clone)]
pub enum WakeSource {
    ApiRequest,
    WakeOnLan,
    ScheduledTask,
    HomeBaseEvent,
    Manual,
}

/// Guard conditions for transitions
#[derive(Debug, Clone)]
pub struct TransitionGuard {
    pub air_gap_compliant: bool,
    pub min_vram_available: Option<u64>,
    pub adapter_healthy: Option<crate::types::AdapterId>,
    pub no_inflight_requests: bool,
    pub temperature_below: Option<f64>,
}

/// Actions to execute during transitions
#[derive(Debug, Clone)]
pub enum TransitionAction {
    PowerGpu(PowerGpuState),
    LoadModel(crate::types::ModelId),
    UnloadModel(crate::types::ModelId),
    StartAdapter(crate::types::AdapterId),
    StopAdapter(crate::types::AdapterId),
    EncryptVault,
    DecryptVault,
    LogTransition,
}

#[derive(Debug, Clone, Copy)]
pub enum PowerGpuState {
    Off,
    LowPower,   // Sentinel mode
    FullPower,  // Full inference
    Throttled,
}

/// Transition definition
#[derive(Debug, Clone)]
pub struct Transition {
    pub from: PowerState,
    pub to: PowerState,
    pub trigger: TransitionTrigger,
    pub guard: Option<TransitionGuard>,
    pub actions: Vec<TransitionAction>,
    pub latency_target: Duration,     // GPU-era
    pub latency_target_qm: Duration,  // QM-era (interface only)
}

/// Power state machine errors
#[derive(Error, Debug)]
pub enum PowerError {
    #[error("Invalid transition: {from:?} -> {to:?} not allowed")]
    InvalidTransition { from: PowerState, to: PowerState },

    #[error("Guard condition failed: {0}")]
    GuardFailed(String),

    #[error("HIL power control failed: {0}")]
    HilPowerError(String),

    #[error("Model load failed during transition: {0}")]
    ModelLoadFailed(String),

    #[error("Transition timeout after {0:?}")]
    TransitionTimeout(Duration),

    #[error("Thermal limit exceeded: {temperature} C")]
    ThermalLimit { temperature: f64 },
}

/// Auto-demotion configuration
#[derive(Debug, Clone)]
pub struct AutoDemotionConfig {
    /// Minutes idle before Full -> Sentinel (default: 12)
    pub full_to_sentinel_minutes: u16,
    /// Minutes idle before Sentinel -> DeepVaultSleep (default: 120)
    pub sentinel_to_deep_minutes: u16,
    /// Whether activity resets the timer
    pub reset_on_activity: bool,
}

/// Power state machine configuration
#[derive(Debug, Clone)]
pub struct PowerConfig {
    pub auto_demotion: AutoDemotionConfig,
    /// GPU temperature threshold for throttling (default: 85.0)
    pub thermal_threshold_celsius: f64,
    /// Temperature for recovery from throttle (default: 75.0)
    pub thermal_recovery_celsius: f64,
    /// Maximum time allowed for any transition (default: 30s)
    pub transition_timeout: Duration,
}

/// Main power state machine
pub struct PowerStateMachine {
    current_state: PowerState,
    config: PowerConfig,
    hil_power_controller: Arc<dyn PowerStateController>,
    auto_demotion_timer: Option<tokio::time::Instant>,
    transition_in_progress: bool,
}

impl PowerStateMachine {
    /// Create new state machine with HIL dependency
    pub fn new(
        config: PowerConfig,
        hil_power_controller: Arc<dyn PowerStateController>,
    ) -> Self {
        Self {
            current_state: PowerState::Off,
            config,
            hil_power_controller,
            auto_demotion_timer: None,
            transition_in_progress: false,
        }
    }

    /// Get current power state
    pub fn current_state(&self) -> PowerState {
        self.current_state
    }

    /// Request a state transition (validates guards, executes actions)
    pub async fn request_transition(
        &mut self,
        trigger: TransitionTrigger,
        target: PowerState,
    ) -> Result<TransitionResult, PowerError> {
        // Implementation in Session 07
        todo!()
    }

    /// Check if auto-demotion should fire
    pub fn check_auto_demotion(&mut self) -> Option<PendingTransition> {
        // Implementation in Session 07
        todo!()
    }

    /// Reset auto-demotion timer (called on API activity)
    pub fn reset_demotion_timer(&mut self) {
        // Implementation in Session 07
        todo!()
    }

    /// Evaluate if request complexity warrants promotion
    pub fn should_promote_to_full(
        &self,
        complexity: ComplexityScore,
        current_capabilities: &crate::registry::CapabilityInfo,
    ) -> bool {
        // Implementation in Session 07
        todo!()
    }

    /// Handle thermal event from HIL
    pub async fn handle_thermal_event(
        &mut self,
        temperature_celsius: f64,
    ) -> Result<(), PowerError> {
        // Implementation in Session 07
        todo!()
    }

    /// Get estimated power draw for current state
    pub fn estimated_power_draw(&self) -> f64 {
        // Implementation in Session 07
        todo!()
    }

    /// Get transition latency target (for dashboard reporting)
    pub fn transition_latency_target(&self, from: PowerState, to: PowerState) -> Option<Duration> {
        // Implementation in Session 07
        todo!()
    }
}

/// Result of transition attempt
#[derive(Debug)]
pub enum TransitionResult {
    Success {
        actual_latency: Duration,
        state: PowerState,
    },
    GuardFailed {
        reason: String,
        current_state: PowerState,
    },
    Timeout {
        target_latency: Duration,
        current_state: PowerState,
    },
}

/// Pending transition (for async handling)
#[derive(Debug)]
pub struct PendingTransition {
    pub from: PowerState,
    pub to: PowerState,
    pub trigger: TransitionTrigger,
    pub estimated_latency: Duration,
}
