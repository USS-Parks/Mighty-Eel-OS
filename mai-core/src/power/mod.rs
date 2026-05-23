//! Power State Machine - Sleep mode management and transition control
//!
//! Implements the five-state power model: Off, DeepVaultSleep, Sentinel,
//! FullInference, ThermalThrottle. Enforces valid transitions, manages
//! auto-demotion timers, and integrates with HIL PowerStateController.

pub mod demotion;
pub mod transitions;

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::types::TransitionId;

/// System power states matching the HIL PowerState enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerState {
    Off,
    DeepVaultSleep,
    Sentinel,
    FullInference,
    ThermalThrottle,
}

impl PowerState {
    pub fn estimated_watts_gpu_era(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::DeepVaultSleep => 2,
            Self::Sentinel => 8,
            Self::FullInference => 350,
            Self::ThermalThrottle => 200,
        }
    }

    pub fn estimated_watts_qm_era(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::DeepVaultSleep => 1,
            Self::Sentinel => 3,
            Self::FullInference => 15,
            Self::ThermalThrottle => 10,
        }
    }

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TransitionTrigger {
    SystemBoot,
    WakeTrigger(WakeSource),
    UrgentWake(WakeSource),
    SentinelPromotion,
    InactivityTimeout,
    ExtendedInactivity,
    ThermalLimitExceeded { temperature_celsius: f32 },
    ThermalRecovery { temperature_celsius: f32 },
    ManualOverride,
    SystemShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeSource {
    ApiRequest,
    WakeOnLan,
    ScheduledTask,
    HomeBaseEvent,
    Manual,
}

#[derive(Debug, Clone)]
pub enum TransitionResult {
    Completed {
        from: PowerState,
        to: PowerState,
        duration: Duration,
    },
    InProgress {
        from: PowerState,
        to: PowerState,
        started_at: Instant,
    },
    Rejected {
        from: PowerState,
        to: PowerState,
        reason: String,
    },
}

#[derive(Error, Debug)]
pub enum PowerError {
    #[error("Invalid transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },
    #[error("Transition guard failed: {0}")]
    GuardFailed(String),
    #[error("Hardware command failed: {0}")]
    HardwareError(String),
    #[error("Invalid timer config: {0}")]
    InvalidTimerConfig(String),
    #[error("Transition timed out: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDemotionConfig {
    pub full_to_sentinel: Duration,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerConfig {
    pub auto_demotion: AutoDemotionConfig,
    pub promotion_latency_target_secs: f32,
    pub thermal_throttle_celsius: f32,
    pub thermal_recovery_celsius: f32,
    pub transition_timeout_secs: f32,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            auto_demotion: AutoDemotionConfig::default(),
            promotion_latency_target_secs: 8.0,
            thermal_throttle_celsius: 83.0,
            thermal_recovery_celsius: 75.0,
            transition_timeout_secs: 30.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRecord {
    pub id: TransitionId,
    pub from: PowerState,
    pub to: PowerState,
    pub trigger: TransitionTrigger,
    pub timestamp_epoch_ms: u64,
    pub duration_ms: Option<u64>,
    pub success: bool,
    pub phase: Option<String>,
}

pub struct PowerStateMachine {
    current_state: PowerState,
    config: PowerConfig,
    last_activity: Instant,
    transition_log: Vec<TransitionRecord>,
    transition_in_progress: bool,
}

impl PowerStateMachine {
    pub fn new(config: PowerConfig) -> Self {
        Self {
            current_state: PowerState::Off,
            config,
            last_activity: Instant::now(),
            transition_log: Vec::new(),
            transition_in_progress: false,
        }
    }

    pub fn with_state(config: PowerConfig, initial_state: PowerState) -> Self {
        Self {
            current_state: initial_state,
            config,
            last_activity: Instant::now(),
            transition_log: Vec::new(),
            transition_in_progress: false,
        }
    }

    pub fn current_state(&self) -> PowerState {
        self.current_state
    }

    pub fn config(&self) -> &PowerConfig {
        &self.config
    }

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
        info!(from = from.as_str(), to = target.as_str(), trigger = ?trigger, "Power state transition");
        self.current_state = target;
        self.last_activity = Instant::now();
        #[allow(clippy::cast_possible_truncation)]
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        #[allow(clippy::cast_possible_truncation)]
        let dur_ms = started.elapsed().as_millis() as u64;
        let record = TransitionRecord {
            id: uuid::Uuid::new_v4(),
            from,
            to: target,
            trigger,
            timestamp_epoch_ms: epoch_ms,
            duration_ms: Some(dur_ms),
            success: true,
            phase: None,
        };
        self.transition_log.push(record);
        Ok(TransitionResult::Completed {
            from,
            to: target,
            duration: started.elapsed(),
        })
    }

    pub fn set_transition_in_progress(&mut self, in_progress: bool) {
        self.transition_in_progress = in_progress;
    }

    pub fn is_transition_in_progress(&self) -> bool {
        self.transition_in_progress
    }

    pub fn check_auto_demotion(&self) -> Option<TransitionTrigger> {
        let idle = self.last_activity.elapsed();
        match self.current_state {
            PowerState::FullInference if idle >= self.config.auto_demotion.full_to_sentinel => {
                debug!(
                    idle_secs = idle.as_secs(),
                    threshold_secs = self.config.auto_demotion.full_to_sentinel.as_secs(),
                    "Auto-demotion: FullInference -> Sentinel"
                );
                Some(TransitionTrigger::InactivityTimeout)
            }
            PowerState::Sentinel if idle >= self.config.auto_demotion.sentinel_to_sleep => {
                debug!(
                    idle_secs = idle.as_secs(),
                    threshold_secs = self.config.auto_demotion.sentinel_to_sleep.as_secs(),
                    "Auto-demotion: Sentinel -> DeepVaultSleep"
                );
                Some(TransitionTrigger::ExtendedInactivity)
            }
            _ => None,
        }
    }

    pub fn reset_demotion_timer(&mut self) {
        self.last_activity = Instant::now();
    }

    pub fn should_promote_to_full(&self, estimated_tokens: u32, is_complex_task: bool) -> bool {
        self.current_state == PowerState::Sentinel && (estimated_tokens > 4096 || is_complex_task)
    }

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

    pub fn estimated_power_draw(&self) -> u32 {
        self.current_state.estimated_watts_gpu_era()
    }

    pub fn transition_latency_target(&self) -> f32 {
        self.config.promotion_latency_target_secs
    }

    pub fn transition_log(&self) -> &[TransitionRecord] {
        &self.transition_log
    }

    pub fn idle_duration(&self) -> Duration {
        self.last_activity.elapsed()
    }

    pub fn resolve_target_state(
        &self,
        trigger: &TransitionTrigger,
    ) -> Result<PowerState, PowerError> {
        match trigger {
            TransitionTrigger::SystemBoot => Ok(PowerState::DeepVaultSleep),
            TransitionTrigger::WakeTrigger(_) => Ok(PowerState::Sentinel),
            TransitionTrigger::UrgentWake(_)
            | TransitionTrigger::SentinelPromotion
            | TransitionTrigger::ThermalRecovery { .. } => Ok(PowerState::FullInference),
            TransitionTrigger::InactivityTimeout => {
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
            TransitionTrigger::ManualOverride => Ok(PowerState::Sentinel),
            TransitionTrigger::SystemShutdown => Ok(PowerState::Off),
        }
    }

    fn is_valid_transition(&self, from: PowerState, to: PowerState) -> bool {
        if to == PowerState::Off {
            return true;
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
        assert_eq!(default_machine().current_state(), PowerState::Off);
    }

    #[test]
    fn test_boot_sequence() {
        let mut psm = default_machine();
        assert!(
            psm.request_transition(TransitionTrigger::SystemBoot)
                .is_ok()
        );
        assert_eq!(psm.current_state(), PowerState::DeepVaultSleep);
    }

    #[test]
    fn test_wake_to_sentinel() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::DeepVaultSleep);
        let r = psm.request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest));
        assert!(r.is_ok());
        assert_eq!(psm.current_state(), PowerState::Sentinel);
    }

    #[test]
    fn test_urgent_wake_to_full() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::DeepVaultSleep);
        assert!(
            psm.request_transition(TransitionTrigger::UrgentWake(WakeSource::Manual))
                .is_ok()
        );
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_sentinel_promotion() {
        let mut psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        assert!(
            psm.request_transition(TransitionTrigger::SentinelPromotion)
                .is_ok()
        );
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_inactivity_demotion() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        assert!(
            psm.request_transition(TransitionTrigger::InactivityTimeout)
                .is_ok()
        );
        assert_eq!(psm.current_state(), PowerState::Sentinel);
    }

    #[test]
    fn test_thermal_throttle() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        let r = psm.handle_thermal_event(85.0).unwrap();
        assert!(r.is_some());
        assert_eq!(psm.current_state(), PowerState::ThermalThrottle);
    }

    #[test]
    fn test_thermal_recovery() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::ThermalThrottle);
        let r = psm.handle_thermal_event(70.0).unwrap();
        assert!(r.is_some());
        assert_eq!(psm.current_state(), PowerState::FullInference);
    }

    #[test]
    fn test_invalid_transition_rejected() {
        let mut psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::Sentinel);
        // Can't go from Sentinel directly to ThermalThrottle
        let r = psm.request_transition(TransitionTrigger::ThermalLimitExceeded {
            temperature_celsius: 90.0,
        });
        assert!(r.is_err());
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
            assert!(
                psm.request_transition(TransitionTrigger::SystemShutdown)
                    .is_ok()
            );
            assert_eq!(psm.current_state(), PowerState::Off);
        }
    }

    #[test]
    fn test_transition_log_recorded() {
        let mut psm = default_machine();
        psm.request_transition(TransitionTrigger::SystemBoot)
            .unwrap();
        psm.request_transition(TransitionTrigger::WakeTrigger(WakeSource::ApiRequest))
            .unwrap();
        assert_eq!(psm.transition_log().len(), 2);
    }

    #[test]
    fn test_estimated_power_draw() {
        let psm = PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        assert_eq!(psm.estimated_power_draw(), 350);
    }

    #[test]
    fn test_reset_demotion_timer() {
        let mut psm =
            PowerStateMachine::with_state(PowerConfig::default(), PowerState::FullInference);
        // Ensure `before` is meaningfully > 0; on fast hosts two
        // `Instant::now()` calls back-to-back can return the same value
        // (monotonic clock granularity) and the post-reset duration
        // would then be nonzero, falsely failing the `<=` check.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let before = psm.idle_duration();
        psm.reset_demotion_timer();
        assert!(
            psm.idle_duration() <= before,
            "idle after reset ({:?}) should be <= idle before reset ({:?})",
            psm.idle_duration(),
            before,
        );
    }
}
