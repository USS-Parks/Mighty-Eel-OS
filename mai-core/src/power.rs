//! Power State Machine - Sleep mode management and transition control
//!
//! Implements the five-state power model: Off, DeepVaultSleep, Sentinel,
//! FullInference, ThermalThrottle. Enforces valid transitions, manages
//! auto-demotion timers, and integrates with HIL PowerStateController.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::types::TransitionId;

/// System power states matching the HIL PowerState enum.
/// Duplicated here for core-level semantics and additional metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerState {
    /// System fully powered off. No wake capability.
    Off,
    /// Deep sleep with vault encryption active. ~2W GPU-era.
    DeepVaultSleep,
    /// Lightweight model loaded for fast-response triage. ~8W GPU-era.
    Sentinel,
    /// Full GPU power for large model inference. ~350W GPU-era.
    FullInference,
    /// Hardware thermal limit exceeded; inference throttled or suspended.
    ThermalThrottle,
}

impl PowerState {
    /// Estimated power draw in watts for GPU-era hardware
    pub fn estimated_watts_gpu_era(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::DeepVaultSleep => 2,
            Self::Sentinel => 8,
            Self::FullInference => 350,
            Self::ThermalThrottle => 200, // throttled but still drawing
        }
    }

    /// Estimated power draw in watts for QM-era hardware (2028+)
    pub fn estimated_watts_qm_era(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::DeepVaultSleep => 1,
            Self::Sentinel => 3,
            Self::FullInference => 15,
            Self::ThermalThrottle => 10,
        }
    }

    /// Display name for logging
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::DeepVaultSleep => "DeepVaultSleep",
            Self::Sentinel => "Sentinel",
            Self::FullInference => "FullInference",
            Self::ThermalThrottle => "ThermalThrottle",
        }
    }
}

/// What triggered the transition request
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TransitionTrigger {
    /// System boot sequence
    SystemBoot,
    /// Wake from deep sleep (various sources)
    WakeTrigger(WakeSource),
    /// Urgent wake bypassing Sentinel (immediate to FullInference)
    UrgentWake(WakeSource),
    /// Request complexity exceeds Sentinel capability
    SentinelPromotion,
    /// Inactivity timer fired (auto-demotion)
    InactivityTimeout,
    /// Extended inactivity (Sentinel -> DeepVaultSleep)
    ExtendedInactivity,
    /// GPU thermal limit exceeded
    ThermalLimitExceeded { temperature_celsius: f32 },
    /// Temperature recovered below threshold
    ThermalRecovery { temperature_celsius: f32 },
    /// Manual operator command
    ManualOverride,
    /// System shutdown requested
    SystemShutdown,
}

/// Source that triggered a wake event
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeSource {
    /// API request received (REST/gRPC)
    ApiRequest,
    /// Wake-on-LAN magic packet (NIC firmware level)
    WakeOnLan,
    /// Scheduled task fired (cron-like)
    ScheduledTask,
    /// HomeBase device event (Matter/Thread)
    HomeBaseEvent,
    /// Manual wake by operator
    Manual,
}

/// Result of a transition attempt
#[derive(Debug, Clone)]
pub enum TransitionResult {
    /// Transition completed successfully
    Completed {
        from: PowerState,
        to: PowerState,
        duration: Duration,
    },
    /// Transition is in progress (async hardware operation)
    InProgress {
        from: PowerState,
        to: PowerState,
        started_at: Instant,
    },
    /// Transition was rejected (invalid or guard failed)
    Rejected {
        from: PowerState,
        to: PowerState,
        reason: String,
    },
}

/// Power state machine errors
#[derive(Error, Debug)]
pub enum PowerError {
    /// Requested transition is not valid from current state
    #[error("Invalid transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    /// Transition guard condition not met
    #[error("Transition guard failed: {0}")]
    GuardFailed(String),

    /// HIL hardware command failed
    #[error("Hardware command failed: {0}")]
    HardwareError(String),

    /// Timer configuration invalid
    #[error("Invalid timer config: {0}")]
    InvalidTimerConfig(String),
}

/// Auto-demotion timer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDemotionConfig {
    /// Time before Full Inference demotes to Sentinel (default: 12 minutes)
    pub full_to_sentinel: Duration,
    /// Time before Sentinel demotes to Deep Vault Sleep (default: 2 hours)
    pub sentinel_to_sleep: Duration,
}

impl Default for AutoDemotionConfig {
    fn default() -> Self {
        Self {
            full_to_sentinel: Duration::from_secs(12 * 60),
            sentinel_to_sleep: Duration::from_secs(2 * 60 * 60),
        }
    }
}

/// Full power configuration including product tier defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerConfig {
    /// Auto-demotion timers
    pub auto_demotion: AutoDemotionConfig,
    /// Target latency for Sentinel-to-Full promotion (seconds)
    pub promotion_latency_target_secs: f32,
    /// Thermal throttle threshold (celsius)
    pub thermal_throttle_celsius: f32,
    /// Thermal recovery threshold (celsius, must be < throttle)
    pub thermal_recovery_celsius: f32,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            auto_demotion: AutoDemotionConfig::default(),
            promotion_latency_target_secs: 8.0,
            thermal_throttle_celsius: 83.0,
            thermal_recovery_celsius: 75.0,
        }
    }
}

/// Audit record for state transitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRecord {
    pub id: TransitionId,
    pub from: PowerState,
    pub to: PowerState,
    pub trigger: TransitionTrigger,
    pub timestamp_epoch_ms: u64,
    pub duration_ms: Option<u64>,
    pub success: bool,
}

/// The power state machine. Manages transitions, timers, and hardware commands.
pub struct PowerStateMachine {
    current_state: PowerState,
    config: PowerConfig,
    /// Timestamp of last activity (used for auto-demotion)
    last_activity: Instant,
    /// History of transitions for audit
    transition_log: Vec<TransitionRecord>,
    /// Whether a transition is currently in progress
    transition_in_progress: bool,
}

impl PowerStateMachine {
    /// Create a new power state machine starting in Off state
    pub fn new(config: PowerConfig) -> Self {
        Self {
            current_state: PowerState::Off,
            config,
            last_activity: Instant::now(),
            transition_log: Vec::new(),
            transition_in_progress: false,
        }
    }

    /// Create with a specified initial state (for testing or recovery)
    pub fn with_state(config: PowerConfig, initial_state: PowerState) -> Self {
        Self {
            current_state: initial_state,
            config,
            last_activity: Instant::now(),
            transition_log: Vec::new(),
            transition_in_progress: false,
        }
    }

    /// Get current power state
    pub fn current_state(&self) -> PowerState {
        self.current_state
    }

    /// Request a state transition. Validates against the transition matrix.
    /// Returns the result of the transition attempt.
    pub fn request_transition(
        &mut self,
        trigger: TransitionTrigger,
    ) -> Result<TransitionResult, PowerError> {
        let target = self.resolve_target_state(&trigger)?;

        if !self.is_valid_transition(self.current_state, target) {
            return Err(PowerError::InvalidTransition {
                from: self.current_state.as_str().to_string(),
                to: target.as_str().to_string(),
            });
        }

        if self.transition_in_progress {
            return Err(PowerError::GuardFailed(
                "Another transition is already in progress".to_string(),
            ));
        }

        let from = self.current_state;
        let started = Instant::now();

        info!(
            from = from.as_str(),
            to = target.as_str(),
            trigger = ?trigger,
            "Power state transition"
        );

        // Execute the transition
        self.current_state = target;
        self.last_activity = Instant::now();

        // Record in audit log
        let record = TransitionRecord {
            id: uuid::Uuid::new_v4(),
            from,
            to: target,
            trigger,
            timestamp_epoch_ms: {
                // Safety: u128 millis since epoch fits in u64 for centuries
                #[allow(clippy::cast_possible_truncation)]
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                ts
            },
            duration_ms: Some({
                // Safety: transition duration millis will never exceed u64
                #[allow(clippy::cast_possible_truncation)]
                let dur = started.elapsed().as_millis() as u64;
                dur
            }),
            success: true,
        };
        self.transition_log.push(record);

        Ok(TransitionResult::Completed {
            from,
            to: target,
            duration: started.elapsed(),
        })
    }

    /// Check if auto-demotion should fire based on elapsed idle time.
    /// Returns the trigger if demotion is due, None otherwise.
    pub fn check_auto_demotion(&self) -> Option<TransitionTrigger> {
        let idle_duration = self.last_activity.elapsed();

        match self.current_state {
            PowerState::FullInference => {
                if idle_duration >= self.config.auto_demotion.full_to_sentinel {
                    debug!(
                        idle_secs = idle_duration.as_secs(),
                        threshold_secs = self.config.auto_demotion.full_to_sentinel.as_secs(),
                        "Auto-demotion: FullInference -> Sentinel"
                    );
                    Some(TransitionTrigger::InactivityTimeout)
                } else {
                    None
                }
            }
            PowerState::Sentinel => {
                if idle_duration >= self.config.auto_demotion.sentinel_to_sleep {
                    debug!(
                        idle_secs = idle_duration.as_secs(),
                        threshold_secs = self.config.auto_demotion.sentinel_to_sleep.as_secs(),
                        "Auto-demotion: Sentinel -> DeepVaultSleep"
                    );
                    Some(TransitionTrigger::ExtendedInactivity)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Reset the demotion timer (called when activity occurs)
    pub fn reset_demotion_timer(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Determine if a request should trigger Sentinel-to-Full promotion.
    /// Based on estimated complexity vs Sentinel capability boundary.
    pub fn should_promote_to_full(&self, estimated_tokens: u32, is_complex_task: bool) -> bool {
        if self.current_state != PowerState::Sentinel {
            return false;
        }
        // Sentinel models are small (3-12B params). Complex tasks or large
        // token counts exceed their capability.
        // Thresholds: >4096 tokens or complex task type
        estimated_tokens > 4096 || is_complex_task
    }

    /// Handle a thermal event from HIL. May trigger throttle or recovery.
    pub fn handle_thermal_event(
        &mut self,
        temperature_celsius: f32,
    ) -> Result<Option<TransitionResult>, PowerError> {
        if temperature_celsius >= self.config.thermal_throttle_celsius
            && self.current_state == PowerState::FullInference
        {
            warn!(
                temp = temperature_celsius,
                threshold = self.config.thermal_throttle_celsius,
                "Thermal throttle triggered"
            );
            let result = self.request_transition(TransitionTrigger::ThermalLimitExceeded {
                temperature_celsius,
            })?;
            Ok(Some(result))
        } else if temperature_celsius <= self.config.thermal_recovery_celsius
            && self.current_state == PowerState::ThermalThrottle
        {
            info!(
                temp = temperature_celsius,
                threshold = self.config.thermal_recovery_celsius,
                "Thermal recovery"
            );
            let result = self.request_transition(TransitionTrigger::ThermalRecovery {
                temperature_celsius,
            })?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Get estimated power draw for current state (GPU-era watts)
    pub fn estimated_power_draw(&self) -> u32 {
        self.current_state.estimated_watts_gpu_era()
    }

    /// Target latency for current promotion path (seconds)
    pub fn transition_latency_target(&self) -> f32 {
        self.config.promotion_latency_target_secs
    }

    /// Get the transition audit log
    pub fn transition_log(&self) -> &[TransitionRecord] {
        &self.transition_log
    }

    /// Time since last activity
    pub fn idle_duration(&self) -> Duration {
        self.last_activity.elapsed()
    }

    // ─── Internal helpers ─────────────────────────────────────────────

    /// Resolve what target state a trigger implies from current state
    fn resolve_target_state(&self, trigger: &TransitionTrigger) -> Result<PowerState, PowerError> {
        match trigger {
            TransitionTrigger::SystemBoot => Ok(PowerState::DeepVaultSleep),
            TransitionTrigger::WakeTrigger(_) => Ok(PowerState::Sentinel),
            TransitionTrigger::UrgentWake(_)
            | TransitionTrigger::SentinelPromotion
            | TransitionTrigger::ThermalRecovery { .. } => Ok(PowerState::FullInference),
            TransitionTrigger::InactivityTimeout => {
                // From FullInference -> Sentinel
                if self.current_state == PowerState::FullInference {
                    Ok(PowerState::Sentinel)
                } else {
                    Err(PowerError::InvalidTransition {
                        from: self.current_state.as_str().to_string(),
                        to: "Sentinel (inactivity)".to_string(),
                    })
                }
            }
            TransitionTrigger::ExtendedInactivity => {
                // From Sentinel -> DeepVaultSleep
                if self.current_state == PowerState::Sentinel {
                    Ok(PowerState::DeepVaultSleep)
                } else {
                    Err(PowerError::InvalidTransition {
                        from: self.current_state.as_str().to_string(),
                        to: "DeepVaultSleep (extended inactivity)".to_string(),
                    })
                }
            }
            TransitionTrigger::ThermalLimitExceeded { .. } => Ok(PowerState::ThermalThrottle),
            TransitionTrigger::ManualOverride => {
                // Manual can go to any adjacent state, but we default to Sentinel
                Ok(PowerState::Sentinel)
            }
            TransitionTrigger::SystemShutdown => Ok(PowerState::Off),
        }
    }

    /// Validate transition against the allowed transition matrix.
    /// Valid transitions (from Session 04 spec):
    ///   Off -> DeepVaultSleep (system boot)
    ///   DeepVaultSleep -> Sentinel (wake trigger)
    ///   DeepVaultSleep -> FullInference (urgent wake)
    ///   Sentinel -> FullInference (promotion)
    ///   Sentinel -> DeepVaultSleep (extended inactivity)
    ///   FullInference -> Sentinel (auto-demotion)
    ///   FullInference -> ThermalThrottle (thermal limit)
    ///   ThermalThrottle -> FullInference (thermal recovery)
    ///   ThermalThrottle -> Sentinel (thermal + demotion)
    ///   Any -> Off (shutdown)
    #[allow(clippy::unused_self)] // self reserved for future guard conditions
    fn is_valid_transition(&self, from: PowerState, to: PowerState) -> bool {
        if to == PowerState::Off {
            return true; // shutdown always valid
        }
        matches!(
            (from, to),
            (
                PowerState::Off | PowerState::Sentinel,
                PowerState::DeepVaultSleep
            ) | (
                PowerState::DeepVaultSleep
                    | PowerState::FullInference
                    | PowerState::ThermalThrottle,
                PowerState::Sentinel
            ) | (
                PowerState::DeepVaultSleep | PowerState::Sentinel | PowerState::ThermalThrottle,
                PowerState::FullInference
            ) | (PowerState::FullInference, PowerState::ThermalThrottle)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_machine() -> PowerStateMachine {
        PowerStateMachine::new(PowerConfig::default())
    }

    #[test]
    fn test_initial_state_is_off() {
        let psm = default_machine();
        assert_eq!(psm.current_state(), PowerState::Off);
    }

    #[test]
    fn test_boot_sequence() {
        let mut psm = default_machine();
        let result = psm.request_transition(TransitionTrigger::SystemBoot);
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::DeepVaultSleep);
    }

    #[test]
    fn test_wake_to_sentinel() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::DeepVaultSleep);
        let result = psm.request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest));
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::Sentinel);
    }

    #[test]
    fn test_urgent_wake_to_full() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::DeepVaultSleep);
        let result = psm.request_transition(TransitionTrigger::UrgentWake(WakeSource::Manual));
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_sentinel_promotion() {
        let mut psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        let result = psm.request_transition(TransitionTrigger::SentinelPromotion);
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_inactivity_demotion() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        let result = psm.request_transition(TransitionTrigger::InactivityTimeout);
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::Sentinel);
    }

    #[test]
    fn test_extended_inactivity_to_sleep() {
        let mut psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        let result = psm.request_transition(TransitionTrigger::ExtendedInactivity);
        assert!(result.is_ok());
        assert_eq!(psm.current_state(), PowerState::DeepVaultSleep);
    }

    #[test]
    fn test_thermal_throttle() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        let result = psm.handle_thermal_event(85.0);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
        assert_eq!(psm.current_state(), PowerState::ThermalThrottle);
    }

    #[test]
    fn test_thermal_recovery() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::ThermalThrottle);
        let result = psm.handle_thermal_event(70.0);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_thermal_no_action_when_normal() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        let result = psm.handle_thermal_event(60.0); // below both thresholds
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // no transition
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_invalid_transition_rejected() {
        let mut psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        // Can't go from Sentinel directly to ThermalThrottle
        let result = psm.request_transition(TransitionTrigger::ThermalLimitExceeded {
            temperature_celsius: 90.0,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_shutdown_from_any_state() {
        for state in [
            PowerState::Off,
            PowerState::DeepVaultSleep,
            PowerState::Sentinel,
            PowerState::FullInference,
            PowerState::ThermalThrottle,
        ] {
            let mut psm = PowerStateMachine::with_state(PowerConfig::default(), state);
            let result = psm.request_transition(TransitionTrigger::SystemShutdown);
            assert!(result.is_ok(), "Shutdown failed from {state:?}");
            assert_eq!(psm.current_state(), PowerState::Off);
        }
    }

    #[test]
    fn test_auto_demotion_check_not_fired() {
        // Fresh machine, timer just reset
        let psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        assert!(psm.check_auto_demotion().is_none());
    }

    #[test]
    fn test_should_promote_to_full() {
        let psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        // Small request: no promotion
        assert!(!psm.should_promote_to_full(100, false));
        // Large token count: promote
        assert!(psm.should_promote_to_full(5000, false));
        // Complex task: promote
        assert!(psm.should_promote_to_full(100, true));
        // Not in sentinel: never promote
        let psm2 = PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        assert!(!psm2.should_promote_to_full(10000, true));
    }

    #[test]
    fn test_transition_log_recorded() {
        let mut psm = default_machine();
        psm.request_transition(TransitionTrigger::SystemBoot)
            .unwrap();
        psm.request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest))
            .unwrap();

        let log = psm.transition_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].from, PowerState::Off);
        assert_eq!(log[0].to, PowerState::DeepVaultSleep);
        assert_eq!(log[1].from, PowerState::DeepVaultSleep);
        assert_eq!(log[1].to, PowerState::Sentinel);
        assert!(log[0].success);
        assert!(log[1].success);
    }

    #[test]
    fn test_estimated_power_draw() {
        let psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::DeepVaultSleep);
        assert_eq!(psm.estimated_power_draw(), 2);

        let psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        assert_eq!(psm.estimated_power_draw(), 350);
    }

    #[test]
    fn test_reset_demotion_timer() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        // Simulate some idle time passing (we can't easily fake Instant, but we can verify reset works)
        let before = psm.idle_duration();
        psm.reset_demotion_timer();
        let after = psm.idle_duration();
        // After reset, idle should be very small (< what it was before, within measurement noise)
        assert!(after <= before);
    }
}
