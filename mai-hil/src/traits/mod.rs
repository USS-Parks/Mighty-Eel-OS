#![deny(unsafe_code)]

mod adapter;
mod hardware_probe;
mod memory_manager;
mod power_state;
mod secure_load;

pub use adapter::{
    AdapterCapabilities, AdapterConfig, AdapterError, AdapterHandle, AdapterMetrics, Embedding,
    FinishReason, GenerationParams, GenerationResult, HealthStatus, Token,
};
pub use hardware_probe::{CapabilityDescriptor, ComputeType, HardwareProbe, QuantizationFormat};
pub use memory_manager::MemoryManager;
pub use power_state::{HardwareEvent, PowerState, PowerStateController};
pub use secure_load::SecureLoadContext;
