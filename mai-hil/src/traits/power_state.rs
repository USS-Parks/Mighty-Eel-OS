use crate::HilError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
// Note: Serialize/Deserialize on HardwareEvent enables tamper-evident audit log
// serialization per the air-gap daemon specification.

/// Power states for the MAI system.
///
/// # Air-Gap and Wake-on-LAN Clarification (Issue #2)
///
/// The air-gap specification states that all NICs are "hard-disabled" during normal
/// operation. This refers to OS-level network interface state: no IP binding, no
/// routing, no TCP/UDP listener, no OS-level packet processing. The air-gap daemon
/// enforces this by administratively downing all interfaces and blocking bind(2).
///
/// Wake-on-LAN (WoL) is NOT an OS-level network service. It operates at the NIC
/// firmware/management controller level (BMC or NIC PHY). The NIC's physical layer
/// remains powered in a low-power listen state even when the OS interface is down,
/// watching for the magic packet pattern (FF FF FF FF FF FF followed by MAC x16).
/// "Port 9" is the conventional UDP port that WoL tools *send* to, but the receiving
/// NIC does not use a port; it pattern-matches raw Ethernet frames at layer 2.
///
/// Therefore: WoL wake capability and air-gap NIC-disabled state are NOT contradictory.
/// The air-gap prevents all OS-initiated and OS-received network traffic. The NIC
/// firmware wake circuit operates independently below the OS network stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PowerState {
    /// System fully powered off. No wake capability.
    Off,
    /// Deep sleep with vault encryption active. NIC firmware WoL listener active
    /// at physical layer (not OS-level). ~2W GPU-era, ~1W QM-era.
    DeepVaultSleep,
    /// Lightweight model loaded for fast-response triage. ~8W GPU-era, ~3W QM-era.
    Sentinel,
    /// Full GPU/accelerator power for large model inference. ~350W GPU-era, ~15W QM-era.
    FullInference,
    /// Hardware thermal limit exceeded; inference throttled or suspended.
    ThermalThrottle,
}

/// Asynchronous hardware events emitted by drivers.
/// Serializable for tamper-evident audit log persistence per air-gap specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
