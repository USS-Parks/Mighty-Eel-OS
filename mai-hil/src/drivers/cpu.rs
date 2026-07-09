//! Generic CPU Fallback Driver Implementation
//!
//! Uses /proc/cpuinfo, /proc/meminfo, and sysfs thermal zones for
//! hardware detection and monitoring. This is the compute target of
//! last resort when no GPU is available.

use crate::HilError;
use crate::drivers::{parse_memory_value, parse_proc_line};
use crate::traits::{
    CapabilityDescriptor, ComputeType, HardwareProbe, MemoryManager, PowerState,
    PowerStateController, QuantizationFormat, SecureLoadContext,
};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::info;

/// CPU fallback driver using procfs and sysfs.
///
/// Parses /proc/cpuinfo for model detection and /proc/meminfo for
/// available system RAM. Thermal monitoring uses sysfs thermal zones.
#[derive(Debug)]
pub struct CpuDriver {
    current_power_state: Mutex<PowerState>,
}

impl CpuDriver {
    /// Create a new CPU driver instance.
    pub fn new() -> Self {
        Self {
            current_power_state: Mutex::new(PowerState::FullInference),
        }
    }

    /// Read and parse /proc/cpuinfo to extract CPU model name.
    async fn read_cpu_model(&self) -> Result<String, HilError> {
        let content = tokio::task::spawn_blocking(|| std::fs::read_to_string("/proc/cpuinfo"))
            .await
            .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
            .map_err(|e| HilError::Unavailable(format!("/proc/cpuinfo unreadable: {e}")))?;

        for line in content.lines() {
            if let Some((key, value)) = parse_proc_line(line)
                && key == "model name"
            {
                return Ok(value.to_string());
            }
        }

        Ok("Unknown CPU".to_string())
    }

    /// Read /proc/meminfo and return (total_bytes, available_bytes).
    async fn read_memory_info(&self) -> Result<(u64, u64), HilError> {
        let content = tokio::task::spawn_blocking(|| std::fs::read_to_string("/proc/meminfo"))
            .await
            .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
            .map_err(|e| HilError::Unavailable(format!("/proc/meminfo unreadable: {e}")))?;

        let mut total_kb: u64 = 0;
        let mut available_kb: u64 = 0;

        for line in content.lines() {
            if let Some((key, value)) = parse_proc_line(line) {
                match key {
                    "MemTotal" => {
                        if let Some(v) = parse_memory_value(value) {
                            total_kb = v;
                        }
                    }
                    "MemAvailable" => {
                        if let Some(v) = parse_memory_value(value) {
                            available_kb = v;
                        }
                    }
                    _ => {}
                }
            }
        }

        let total_bytes = total_kb * 1024;
        let available_bytes = available_kb * 1024;
        Ok((total_bytes, available_bytes))
    }

    /// Detect CPU SIMD capabilities from /proc/cpuinfo flags.
    async fn detect_compute_types(&self) -> Vec<ComputeType> {
        let content = tokio::task::spawn_blocking(|| std::fs::read_to_string("/proc/cpuinfo"))
            .await
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or_else(String::new);

        let mut types = vec![ComputeType::CPUFallback];

        for line in content.lines() {
            if let Some((key, value)) = parse_proc_line(line)
                && key == "flags"
            {
                if value.contains("avx512f") || value.contains("avx512_bf16") {
                    types.push(ComputeType::BF16);
                    types.push(ComputeType::INT8);
                } else if value.contains("avx2") {
                    types.push(ComputeType::INT8);
                }
                break; // flags are same for all cores
            }
        }

        types
    }

    /// Read thermal zone temperature from sysfs.
    async fn read_thermal_zone(&self) -> Result<f32, HilError> {
        let content = tokio::task::spawn_blocking(|| {
            std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        })
        .await
        .map_err(|e| HilError::Unavailable(format!("Task join error: {e}")))?
        .map_err(|e| HilError::Unavailable(format!("Thermal zone unreadable: {e}")))?;

        // sysfs reports in millidegrees
        let millideg: f32 = content
            .trim()
            .parse()
            .map_err(|_| HilError::Unavailable("Cannot parse thermal value".to_string()))?;

        Ok(millideg / 1000.0)
    }
}

impl Default for CpuDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareProbe for CpuDriver {
    async fn discover_devices(&self) -> Result<Vec<CapabilityDescriptor>, HilError> {
        let model_name = self.read_cpu_model().await?;
        let (total_bytes, _available) = self.read_memory_info().await?;
        let compute_types = self.detect_compute_types().await;

        let descriptor = CapabilityDescriptor {
            model_name,
            total_memory_bytes: total_bytes,
            compute_capabilities: compute_types,
            bandwidth_gbps: 0.0, // CPU memory bandwidth varies; not easily queried
            tdp_watts: 65,       // Conservative default for server CPUs
            thermal_threshold_celsius: 100.0,
            driver_version: "procfs".to_string(),
            supported_quantizations: vec![
                QuantizationFormat::GGUF,
                QuantizationFormat::SafeTensors,
            ],
            hot_pluggable: false,
        };

        Ok(vec![descriptor])
    }

    async fn get_thermal_state(&self, _device_uuid: &str) -> Result<f32, HilError> {
        self.read_thermal_zone().await
    }
}

#[async_trait]
impl PowerStateController for CpuDriver {
    async fn set_power_state(&self, state: PowerState) -> Result<(), HilError> {
        info!("CPU: power state -> {:?}", state);
        let mut current = self.current_power_state.lock().await;
        *current = state;
        Ok(())
    }

    async fn get_power_state(&self) -> Result<PowerState, HilError> {
        let current = self.current_power_state.lock().await;
        Ok(current.clone())
    }

    async fn set_thermal_limit(&self, limit_celsius: f32) -> Result<(), HilError> {
        if !(30.0..=105.0).contains(&limit_celsius) {
            return Err(HilError::ThermalLimitExceeded {
                temperature: limit_celsius,
            });
        }
        info!("CPU: thermal limit set to {}C (advisory)", limit_celsius);
        Ok(())
    }
}

#[async_trait]
impl MemoryManager for CpuDriver {
    async fn allocate_memory(&self, size_bytes: u64) -> Result<u64, HilError> {
        let (total, used) = self.get_memory_usage().await?;
        let available = total.saturating_sub(used);
        if size_bytes > available {
            return Err(HilError::OutOfMemory {
                requested: size_bytes,
                available,
            });
        }
        Ok(used)
    }

    async fn free_memory(&self, _handle: u64) -> Result<(), HilError> {
        Ok(())
    }

    async fn predict_fit(&self, required_size: u64) -> Result<bool, HilError> {
        let (total, used) = self.get_memory_usage().await?;
        let available = total.saturating_sub(used);
        Ok(required_size <= available)
    }

    async fn get_memory_usage(&self) -> Result<(u64, u64), HilError> {
        let (total_bytes, available_bytes) = self.read_memory_info().await?;
        let used_bytes = total_bytes.saturating_sub(available_bytes);
        // Return (total, used) per trait contract
        Ok((total_bytes, used_bytes))
    }
}

#[async_trait]
impl SecureLoadContext for CpuDriver {
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

    #[tokio::test]
    async fn test_cpu_driver_default() {
        let driver = CpuDriver::default();
        let state = driver.get_power_state().await.unwrap();
        assert_eq!(state, PowerState::FullInference);
    }

    #[tokio::test]
    async fn test_power_state_round_trip() {
        let driver = CpuDriver::new();
        driver.set_power_state(PowerState::Sentinel).await.unwrap();
        let state = driver.get_power_state().await.unwrap();
        assert_eq!(state, PowerState::Sentinel);
    }

    #[tokio::test]
    async fn test_thermal_limit_bounds() {
        let driver = CpuDriver::new();
        // Too high
        assert!(driver.set_thermal_limit(200.0).await.is_err());
        // Too low
        assert!(driver.set_thermal_limit(10.0).await.is_err());
        // Valid
        assert!(driver.set_thermal_limit(80.0).await.is_ok());
    }
}
