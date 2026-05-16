use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use crate::HilError;

/// Power states for the MAI system
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PowerState {
    Off,
    DeepVaultSleep, // ~2W GPU-era, ~1W QM-era
    Sentinel,       // ~8W GPU-era, ~3W QM-era
    FullInference,  // ~350W GPU-era, ~15W QM-era
    ThermalThrottle,
}

/// Asynchronous hardware events emitted by drivers
#[derive(Debug, Clone)]
pub enum HardwareEvent {
    DeviceAdded { device_id: String },
    DeviceRemoved { device_id: String },
    ThermalStateChange { temperature: f32, device_id: String },
    PowerStateTransitionRequested { from: PowerState, to: PowerState },
}

/// `PowerStateController`: Interface for managing hardware power transitions and limits.
/// Core Kernel uses this to command sleep/wake cycles and thermal throttling.
#[async_trait]
pub trait PowerStateController: Send + Sync {
    /// Transitions hardware to the requested power state.
    async fn set_power_state(&self, state: PowerState) -> Result<(), HilError>;

    /// Returns the current hardware power state.
    async fn get_power_state(&self) -> Result<PowerState, HilError>;

    /// Configures hardware-level thermal limits. Returns error if limit exceeds TDP.
    async fn set_thermal_limit(&self, limit_celsius: f32) -> Result<(), HilError>;
}
