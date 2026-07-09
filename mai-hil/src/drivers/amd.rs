//! AMD ROCm Driver Implementation
//!
//! Uses `rocm-smi` CLI via tokio::process::Command for GPU discovery,
//! thermal monitoring, power management, and memory tracking.
//! No direct library dependency - works with any ROCm version that ships rocm-smi.

use crate::HilError;
use crate::traits::{
    CapabilityDescriptor, ComputeType, HardwareProbe, MemoryManager, PowerState,
    PowerStateController, QuantizationFormat, SecureLoadContext,
};
use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::info;

/// AMD GPU driver backed by rocm-smi CLI.
///
/// Shell-outs to `rocm-smi --showallinfo --json` for hardware queries.
/// This avoids linking to HIP/ROCm libraries directly.
#[derive(Debug)]
pub struct AmdDriver {
    device_index: u32,
    current_power_state: Mutex<PowerState>,
}

impl AmdDriver {
    /// Create a new AMD driver instance for the given device index.
    pub fn new(device_index: u32) -> Self {
        Self {
            device_index,
            current_power_state: Mutex::new(PowerState::FullInference),
        }
    }

    /// Execute rocm-smi and return parsed JSON output.
    async fn run_rocm_smi(&self, args: &[&str]) -> Result<serde_json::Value, HilError> {
        let output = Command::new("rocm-smi")
            .args(args)
            .arg("--json")
            .output()
            .await
            .map_err(|e| HilError::Unavailable(format!("rocm-smi not found or failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(HilError::Unavailable(format!(
                "rocm-smi exited with error: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(&stdout)
            .map_err(|e| HilError::Unavailable(format!("Failed to parse rocm-smi JSON: {e}")))
    }

    /// Extract the card key for this device index from rocm-smi JSON.
    fn card_key(&self) -> String {
        format!("card{}", self.device_index)
    }
}

impl Default for AmdDriver {
    fn default() -> Self {
        Self::new(0)
    }
}

#[async_trait]
impl HardwareProbe for AmdDriver {
    async fn discover_devices(&self) -> Result<Vec<CapabilityDescriptor>, HilError> {
        let json = self
            .run_rocm_smi(&["--showproductname", "--showmeminfo", "vram"])
            .await?;

        let card_key = self.card_key();
        let empty_map = serde_json::Map::new();
        let card = json
            .as_object()
            .unwrap_or(&empty_map)
            .get(&card_key)
            .and_then(|v| v.as_object());

        let Some(card) = card else {
            return Err(HilError::Unavailable(format!(
                "AMD GPU {} not found in rocm-smi output",
                self.device_index
            )));
        };

        let model_name = card
            .get("Card Series")
            .or_else(|| card.get("Card series"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown AMD GPU")
            .to_string();

        // VRAM total in bytes (rocm-smi reports in bytes or MB depending on version)
        let total_memory = card
            .get("VRAM Total Memory (B)")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let compute_types = vec![ComputeType::FP16, ComputeType::INT8, ComputeType::BF16];

        let supported_quants = vec![QuantizationFormat::GGUF, QuantizationFormat::SafeTensors];

        let descriptor = CapabilityDescriptor {
            model_name,
            total_memory_bytes: total_memory,
            compute_capabilities: compute_types,
            bandwidth_gbps: 0.0, // rocm-smi doesn't expose this reliably
            tdp_watts: 300,      // Conservative default; real TDP from --showpower
            thermal_threshold_celsius: 90.0,
            driver_version: "rocm-smi".to_string(),
            supported_quantizations: supported_quants,
            hot_pluggable: false,
        };

        Ok(vec![descriptor])
    }

    async fn get_thermal_state(&self, _device_uuid: &str) -> Result<f32, HilError> {
        let json = self.run_rocm_smi(&["--showtemp"]).await?;

        let card_key = self.card_key();
        let empty_map = serde_json::Map::new();
        let card = json
            .as_object()
            .unwrap_or(&empty_map)
            .get(&card_key)
            .and_then(|v| v.as_object());

        let Some(card) = card else {
            return Err(HilError::Unavailable(
                "Temperature data unavailable".to_string(),
            ));
        };

        // rocm-smi reports "Temperature (Sensor edge) (C)" or similar
        let temp = card
            .get("Temperature (Sensor edge) (C)")
            .or_else(|| card.get("Temperature (Sensor junction) (C)"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f32>().ok())
            .or_else(|| {
                card.get("Temperature (Sensor edge) (C)")
                    .and_then(serde_json::Value::as_f64)
                    .map(|f| {
                        #[allow(clippy::cast_possible_truncation)]
                        let temp = f as f32;
                        temp
                    })
            })
            .unwrap_or(0.0);

        Ok(temp)
    }
}

#[async_trait]
impl PowerStateController for AmdDriver {
    async fn set_power_state(&self, state: PowerState) -> Result<(), HilError> {
        info!("AMD GPU {}: power state -> {:?}", self.device_index, state);
        let mut current = self.current_power_state.lock().await;
        *current = state;
        Ok(())
    }

    async fn get_power_state(&self) -> Result<PowerState, HilError> {
        let current = self.current_power_state.lock().await;
        Ok(current.clone())
    }

    async fn set_thermal_limit(&self, limit_celsius: f32) -> Result<(), HilError> {
        if !(30.0..=100.0).contains(&limit_celsius) {
            return Err(HilError::ThermalLimitExceeded {
                temperature: limit_celsius,
            });
        }
        info!(
            "AMD GPU {}: thermal limit set to {}C (advisory)",
            self.device_index, limit_celsius
        );
        Ok(())
    }
}

#[async_trait]
impl MemoryManager for AmdDriver {
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
        let json = self.run_rocm_smi(&["--showmeminfo", "vram"]).await?;

        let card_key = self.card_key();
        let empty_map = serde_json::Map::new();
        let card = json
            .as_object()
            .unwrap_or(&empty_map)
            .get(&card_key)
            .and_then(|v| v.as_object());

        let Some(card) = card else {
            return Err(HilError::Unavailable("VRAM info unavailable".to_string()));
        };

        let total = card
            .get("VRAM Total Memory (B)")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let used = card
            .get("VRAM Total Used Memory (B)")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Return (total, used) per trait contract
        Ok((total, used))
    }
}

#[async_trait]
impl SecureLoadContext for AmdDriver {
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

    #[test]
    fn test_amd_driver_default() {
        let driver = AmdDriver::default();
        assert_eq!(driver.device_index, 0);
    }

    #[test]
    fn test_card_key_format() {
        let driver = AmdDriver::new(2);
        assert_eq!(driver.card_key(), "card2");
    }

    #[tokio::test]
    async fn test_power_state_round_trip() {
        let driver = AmdDriver::new(0);
        driver.set_power_state(PowerState::Sentinel).await.unwrap();
        let state = driver.get_power_state().await.unwrap();
        assert_eq!(state, PowerState::Sentinel);
    }
}
