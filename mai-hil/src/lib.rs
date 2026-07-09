#![doc = "Island Mountain MAI - Hardware Interface Layer (HIL)"]
// unsafe_code is explicitly denied in trait modules.
// Driver implementations (mai-hil/src/drivers/) are permitted to use unsafe
// where direct hardware/FFI access is required per CONVENTIONS.md.

pub mod drivers;
pub mod traits;

/// Re-export core traits for MAI Core consumption
pub use traits::{
    CapabilityDescriptor, ComputeType, HardwareEvent, HardwareProbe, MemoryManager, PowerState,
    PowerStateController, QuantizationFormat, SecureLoadContext,
};

/// HIL Error Enumeration
#[derive(Debug, thiserror::Error)]
pub enum HilError {
    #[error("Operation not implemented for this hardware class")]
    NotImplemented,

    #[error("Hardware unavailable: {0}")]
    Unavailable(String),

    #[error("Memory allocation failed: requested {requested}, available {available}")]
    OutOfMemory { requested: u64, available: u64 },

    #[error("TPM attestation failed: {0}")]
    TpmAttestationFailed(String),

    #[error("Thermal limit exceeded: {temperature}C")]
    ThermalLimitExceeded { temperature: f32 },

    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
}
