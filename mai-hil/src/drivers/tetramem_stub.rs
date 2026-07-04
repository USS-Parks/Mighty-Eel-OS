use crate::HilError;
use crate::traits::{
    CapabilityDescriptor, HardwareProbe, MemoryManager, PowerState, PowerStateController,
    SecureLoadContext,
};
use async_trait::async_trait;

/// Stub TetraMem MX100 Driver Implementation
///
/// This is a skeletal implementation that satisfies the compiler and trait contracts.
/// All methods return `HilError::NotImplemented` to indicate that real TetraMem SDK
/// integration will be implemented in Phase 3. The struct compiles cleanly.
#[derive(Debug)]
pub struct TetraMemDriver {
    pub device_index: u32,
}

impl TetraMemDriver {
    #[must_use]
    pub fn new() -> Self {
        Self { device_index: 0 }
    }
}

impl Default for TetraMemDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareProbe for TetraMemDriver {
    async fn discover_devices(&self) -> Result<Vec<CapabilityDescriptor>, HilError> {
        Err(HilError::NotImplemented)
    }

    async fn get_thermal_state(&self, _device_uuid: &str) -> Result<f32, HilError> {
        Err(HilError::NotImplemented)
    }
}

#[async_trait]
impl PowerStateController for TetraMemDriver {
    async fn set_power_state(&self, _state: PowerState) -> Result<(), HilError> {
        Err(HilError::NotImplemented)
    }

    async fn get_power_state(&self) -> Result<PowerState, HilError> {
        Err(HilError::NotImplemented)
    }

    async fn set_thermal_limit(&self, _limit_celsius: f32) -> Result<(), HilError> {
        Err(HilError::NotImplemented)
    }
}

#[async_trait]
impl MemoryManager for TetraMemDriver {
    async fn allocate_memory(&self, _size_bytes: u64) -> Result<u64, HilError> {
        Err(HilError::NotImplemented)
    }

    async fn free_memory(&self, _handle: u64) -> Result<(), HilError> {
        Err(HilError::NotImplemented)
    }

    async fn predict_fit(&self, _required_size: u64) -> Result<bool, HilError> {
        Err(HilError::NotImplemented)
    }

    async fn get_memory_usage(&self) -> Result<(u64, u64), HilError> {
        Err(HilError::NotImplemented)
    }
}

#[async_trait]
impl SecureLoadContext for TetraMemDriver {
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
    async fn test_tetramem_stub_returns_not_implemented() {
        let driver = TetraMemDriver::new();

        assert!(driver.discover_devices().await.is_err());
        assert!(
            driver
                .set_power_state(PowerState::FullInference)
                .await
                .is_err()
        );
        assert!(driver.allocate_memory(1024).await.is_err());
        assert!(driver.unseal_tpm_key().await.is_err());
    }
}
