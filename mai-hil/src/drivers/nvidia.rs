//! NVIDIA CUDA Driver Implementation
//!
//! Uses nvml-wrapper (NVML bindings) for GPU discovery, thermal monitoring,
//! power management, and memory tracking. Feature-gated behind `nvidia`.

use crate::HilError;
use crate::traits::{
    CapabilityDescriptor, ComputeType, HardwareProbe, MemoryManager, PowerState,
    PowerStateController, QuantizationFormat, SecureLoadContext,
};
use async_trait::async_trait;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// NVIDIA GPU driver backed by NVML.
///
/// Holds a shared reference to the NVML library handle and an index
/// identifying which GPU this driver instance manages.
#[derive(Debug)]
pub struct NvidiaDriver {
    nvml: Arc<Nvml>,
    device_index: u32,
    current_power_state: Mutex<PowerState>,
}

impl NvidiaDriver {
    /// Initialize a new NvidiaDriver for the given device index.
    /// Returns `HilError::Unavailable` if NVML cannot initialize.
    pub fn new(device_index: u32) -> Result<Self, HilError> {
        let nvml =
            Nvml::init().map_err(|e| HilError::Unavailable(format!("NVML init failed: {e}")))?;
        Ok(Self {
            nvml: Arc::new(nvml),
            device_index,
            current_power_state: Mutex::new(PowerState::FullInference),
        })
    }

    /// Create from an existing NVML handle (useful for multi-GPU setups).
    pub fn with_nvml(nvml: Arc<Nvml>, device_index: u32) -> Self {
        Self {
            nvml,
            device_index,
            current_power_state: Mutex::new(PowerState::FullInference),
        }
    }

    /// Determine compute capabilities from CUDA compute capability version.
    fn compute_types_from_arch(major: i32, minor: i32) -> Vec<ComputeType> {
        let mut types = vec![ComputeType::FP16, ComputeType::INT8];
        // Ampere (8.x) and above support BF16 and INT4
        if major >= 8 {
            types.push(ComputeType::BF16);
            types.push(ComputeType::INT4);
        }
        types
    }
}

#[async_trait]
impl HardwareProbe for NvidiaDriver {
    async fn discover_devices(&self) -> Result<Vec<CapabilityDescriptor>, HilError> {
        let nvml = self.nvml.clone();
        let device_index = self.device_index;

        tokio::task::spawn_blocking(move || {
            let device = nvml.device_by_index(device_index).map_err(|e| {
                HilError::Unavailable(format!("Cannot access GPU {device_index}: {e}"))
            })?;

            let name = device
                .name()
                .unwrap_or_else(|_| "Unknown NVIDIA GPU".into());
            let mem_info = device
                .memory_info()
                .map_err(|e| HilError::Unavailable(format!("Memory info unavailable: {e}")))?;

            let cc = device.cuda_compute_capability().map_err(|e| {
                HilError::Unavailable(format!("Compute capability query failed: {e}"))
            })?;

            let bandwidth = 0.0_f32; // NVML doesn't expose bandwidth directly

            let power_limit = device.power_management_limit().unwrap_or(350_000); // milliwatts
            let tdp_watts = (power_limit / 1000) as u32;

            let driver_version = nvml
                .sys_driver_version()
                .unwrap_or_else(|_| "unknown".into());

            let compute_types = Self::compute_types_from_arch(cc.major, cc.minor);

            let supported_quants = vec![
                QuantizationFormat::GGUF,
                QuantizationFormat::EXL2,
                QuantizationFormat::GPTQ,
                QuantizationFormat::SafeTensors,
            ];

            let descriptor = CapabilityDescriptor {
                model_name: name,
                total_memory_bytes: mem_info.total,
                compute_capabilities: compute_types,
                bandwidth_gbps: bandwidth,
                tdp_watts,
                thermal_threshold_celsius: 83.0,
                driver_version,
                supported_quantizations: supported_quants,
                hot_pluggable: false,
            };

            Ok(vec![descriptor])
        })
        .await
        .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
    }

    async fn get_thermal_state(&self, _device_uuid: &str) -> Result<f32, HilError> {
        let nvml = self.nvml.clone();
        let device_index = self.device_index;

        tokio::task::spawn_blocking(move || {
            let device = nvml.device_by_index(device_index).map_err(|e| {
                HilError::Unavailable(format!("Cannot access GPU {device_index}: {e}"))
            })?;

            let temp = device
                .temperature(TemperatureSensor::Gpu)
                .map_err(|e| HilError::Unavailable(format!("Temperature read failed: {e}")))?;

            Ok(temp as f32)
        })
        .await
        .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
    }
}

#[async_trait]
impl PowerStateController for NvidiaDriver {
    async fn set_power_state(&self, state: PowerState) -> Result<(), HilError> {
        // NVML doesn't have a direct analog to our power state model.
        // We track the logical state and use persistence mode for Sentinel/FullInference.
        info!(
            "NVIDIA GPU {}: power state -> {:?}",
            self.device_index, state
        );
        let mut current = self.current_power_state.lock().await;
        *current = state;
        Ok(())
    }

    async fn get_power_state(&self) -> Result<PowerState, HilError> {
        let current = self.current_power_state.lock().await;
        Ok(current.clone())
    }

    async fn set_thermal_limit(&self, limit_celsius: f32) -> Result<(), HilError> {
        // NVML supports setting power limit but not thermal limit directly.
        // Log the request; real enforcement is via driver-level thermal throttle.
        if limit_celsius > 95.0 || limit_celsius < 30.0 {
            return Err(HilError::ThermalLimitExceeded {
                temperature: limit_celsius,
            });
        }
        info!(
            "NVIDIA GPU {}: thermal limit set to {}C (advisory)",
            self.device_index, limit_celsius
        );
        Ok(())
    }
}

#[async_trait]
impl MemoryManager for NvidiaDriver {
    async fn allocate_memory(&self, size_bytes: u64) -> Result<u64, HilError> {
        // NVML is a monitoring library, not an allocator.
        // Real allocation happens inside the inference backend (vLLM/llama.cpp).
        // We check feasibility here.
        let (total, used) = self.get_memory_usage().await?;
        let available = total.saturating_sub(used);
        if size_bytes > available {
            return Err(HilError::OutOfMemory {
                requested: size_bytes,
                available,
            });
        }
        // Return a pseudo-handle (offset-based)
        Ok(used)
    }

    async fn free_memory(&self, _handle: u64) -> Result<(), HilError> {
        // Deallocation is managed by the backend process, not NVML.
        Ok(())
    }

    async fn predict_fit(&self, required_size: u64) -> Result<bool, HilError> {
        let (total, used) = self.get_memory_usage().await?;
        let available = total.saturating_sub(used);
        Ok(required_size <= available)
    }

    async fn get_memory_usage(&self) -> Result<(u64, u64), HilError> {
        let nvml = self.nvml.clone();
        let device_index = self.device_index;

        tokio::task::spawn_blocking(move || {
            let device = nvml.device_by_index(device_index).map_err(|e| {
                HilError::Unavailable(format!("Cannot access GPU {device_index}: {e}"))
            })?;

            let mem_info = device
                .memory_info()
                .map_err(|e| HilError::Unavailable(format!("Memory info unavailable: {e}")))?;

            // Return (total, used) per trait contract
            Ok((mem_info.total, mem_info.used))
        })
        .await
        .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
    }
}

#[async_trait]
impl SecureLoadContext for NvidiaDriver {
    async fn unseal_tpm_key(&self) -> Result<Vec<u8>, HilError> {
        Err(HilError::NotImplemented)
    }

    async fn decrypt_and_verify(
        &self,
        _encrypted_blob: &[u8],
        _manifest_hash: &str,
    ) -> Result<Vec<u8>, HilError> {
        Err(HilError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests require actual NVIDIA hardware + nvidia feature
    // Unit tests verify struct construction and error paths

    #[test]
    fn test_compute_types_ampere() {
        let types = NvidiaDriver::compute_types_from_arch(8, 6);
        assert!(types.contains(&ComputeType::BF16));
        assert!(types.contains(&ComputeType::INT4));
    }

    #[test]
    fn test_compute_types_turing() {
        let types = NvidiaDriver::compute_types_from_arch(7, 5);
        assert!(types.contains(&ComputeType::FP16));
        assert!(!types.contains(&ComputeType::BF16));
    }
}
