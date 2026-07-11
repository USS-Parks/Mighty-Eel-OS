use crate::HilError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Represents the type of computation the hardware accelerates
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComputeType {
    FP16,
    INT8,
    INT4,
    BF16,
    CPUFallback,
}

/// Supported model weight formats
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuantizationFormat {
    GGUF,
    EXL2,
    GPTQ,
    SafeTensors,
}

/// Hardware capability descriptor returned by all HIL drivers
/// Generic enough to describe both classical GPUs and future quantum memristor SoCs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Silicon/model identifier
    pub model_name: String,
    /// Total usable compute memory in bytes
    pub total_memory_bytes: u64,
    /// Supported precision/types
    pub compute_capabilities: Vec<ComputeType>,
    /// Peak memory bandwidth in GB/s
    pub bandwidth_gbps: f32,
    /// Thermal Design Power in watts
    pub tdp_watts: u32,
    /// Maximum safe operating temperature in Celsius
    pub thermal_threshold_celsius: f32,
    /// Driver/SDK version string
    pub driver_version: String,
    /// Supported quantization formats
    pub supported_quantizations: Vec<QuantizationFormat>,
    /// True if hardware can be hot-plugged without reboot
    pub hot_pluggable: bool,
}

impl Default for CapabilityDescriptor {
    fn default() -> Self {
        Self {
            model_name: String::from("Unknown"),
            total_memory_bytes: 0,
            compute_capabilities: vec![],
            bandwidth_gbps: 0.0,
            tdp_watts: 0,
            thermal_threshold_celsius: 85.0,
            driver_version: String::from("0.0.0"),
            supported_quantizations: vec![],
            hot_pluggable: false,
        }
    }
}

/// `HardwareProbe`: Standardized interface for hardware detection and capability reporting.
/// Implemented by NVIDIA, AMD, and CPU drivers.
#[async_trait]
pub trait HardwareProbe: Send + Sync {
    /// Detects and enumerates all available hardware of this driver type.
    /// Returns a vector of capability descriptors.
    async fn discover_devices(&self) -> Result<Vec<CapabilityDescriptor>, HilError>;

    /// Returns the current thermal state for a specific device UUID.
    async fn get_thermal_state(&self, device_uuid: &str) -> Result<f32, HilError>;
}
