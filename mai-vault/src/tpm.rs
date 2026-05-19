//! TPM 2.0 key management.
//!
//! Implements `TpmProvider` for hardware-backed key sealing and unsealing.
//! Keys are bound to PCR (Platform Configuration Register) state, ensuring
//! that sealed keys are only accessible when the system boot chain is intact.
//!
//! # PCR Binding
//!
//! By default, keys are sealed to PCRs 0 and 7:
//! - PCR 0: BIOS/firmware measurement
//! - PCR 7: Secure Boot state
//!
//! If the firmware is updated or Secure Boot configuration changes, sealed
//! keys become inaccessible. An admin must re-seal keys after verifying
//! the new system state.
//!
//! # Stub Status
//!
//! This implementation provides in-memory key storage that simulates TPM
//! behavior. Real TPM operations require the `tss-esapi` crate and a
//! TPM 2.0 device at `/dev/tpmrm0`.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{KeyInfo, KeyLevel, TpmProvider, VaultError};

use crate::config::TpmConfig;

/// TPM 2.0 key management implementation.
///
/// In stub mode, keys are stored in memory with a simulated PCR state.
/// The `seal_key` and `unseal_key` methods verify PCR consistency but
/// do not interact with actual TPM hardware.
pub struct TpmManager {
    config: TpmConfig,
    /// Simulated PCR state (hash of PCR values at seal time).
    current_pcr_state: RwLock<Vec<u8>>,
    /// Sealed keys: key_id -> (sealed_blob, pcr_state_at_seal_time)
    sealed_keys: RwLock<HashMap<String, SealedKeyEntry>>,
    /// Whether a TPM device is available.
    available: bool,
}

/// A sealed key entry in the simulated TPM.
struct SealedKeyEntry {
    /// The key data, "encrypted" under the PCR state.
    sealed_blob: Vec<u8>,
    /// PCR state at the time of sealing.
    pcr_state: Vec<u8>,
    /// Key metadata.
    info: KeyInfo,
}

impl TpmManager {
    /// Create a new TPM manager.
    ///
    /// If `config.required` is true and no TPM is found, operations will
    /// return `TpmUnavailable`. If false, software fallback is used.
    pub fn new(config: TpmConfig) -> Self {
        // In production: probe /dev/tpmrm0 to check availability.
        // In stub mode: always "available" unless config says otherwise.
        let available = !config.required || cfg!(test);

        Self {
            config,
            current_pcr_state: RwLock::new(Self::compute_initial_pcr_state()),
            sealed_keys: RwLock::new(HashMap::new()),
            available,
        }
    }

    /// Compute an initial PCR state (deterministic for testing).
    fn compute_initial_pcr_state() -> Vec<u8> {
        // In production: read actual PCR values from TPM.
        // Stub: use a fixed "healthy boot" state.
        blake3::hash(b"healthy-boot-pcr-0-7")
            .as_bytes()
            .to_vec()
    }

    /// Simulate a PCR state change (for testing tamper scenarios).
    #[cfg(test)]
    pub async fn simulate_pcr_change(&self) {
        let mut state = self.current_pcr_state.write().await;
        *state = blake3::hash(b"modified-firmware-pcr-changed")
            .as_bytes()
            .to_vec();
    }

    /// Reset PCR state to initial (for testing recovery).
    #[cfg(test)]
    pub async fn reset_pcr_state(&self) {
        let mut state = self.current_pcr_state.write().await;
        *state = Self::compute_initial_pcr_state();
    }

    /// "Seal" key data by XOR with PCR state (stub encryption).
    fn seal_with_pcr(key_data: &[u8], pcr_state: &[u8]) -> Vec<u8> {
        let mut sealed = key_data.to_vec();
        for (i, byte) in sealed.iter_mut().enumerate() {
            *byte ^= pcr_state[i % pcr_state.len()];
        }
        sealed
    }

    /// "Unseal" key data by XOR with PCR state (stub decryption).
    fn unseal_with_pcr(sealed_data: &[u8], pcr_state: &[u8]) -> Vec<u8> {
        // XOR is its own inverse
        Self::seal_with_pcr(sealed_data, pcr_state)
    }
}

#[async_trait]
impl TpmProvider for TpmManager {
    async fn is_available(&self) -> bool {
        self.available
    }

    async fn seal_key(
        &self,
        key_data: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        let pcr_state = self.current_pcr_state.read().await.clone();
        let sealed_blob = Self::seal_with_pcr(key_data, &pcr_state);

        info!(key_id, blob_len = sealed_blob.len(), "Key sealed to TPM PCR state");

        let mut keys = self.sealed_keys.write().await;
        keys.insert(
            key_id.to_string(),
            SealedKeyEntry {
                sealed_blob: sealed_blob.clone(),
                pcr_state,
                info: KeyInfo {
                    key_id: key_id.to_string(),
                    level: KeyLevel::Master,
                    algorithm: "TPM-SEAL".to_string(),
                    created_at: chrono::Utc::now().timestamp() as u64,
                    model_id: None,
                    tpm_sealed: true,
                },
            },
        );

        Ok(sealed_blob)
    }

    async fn unseal_key(
        &self,
        sealed_blob: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        let keys = self.sealed_keys.read().await;
        let entry = keys
            .get(key_id)
            .ok_or_else(|| VaultError::TpmError(format!("Key not found: {}", key_id)))?;

        // Verify PCR state matches seal-time state
        let current_pcr = self.current_pcr_state.read().await;
        if *current_pcr != entry.pcr_state {
            warn!(
                key_id,
                "PCR state mismatch - system boot chain may have changed"
            );
            return Err(VaultError::TpmPcrMismatch);
        }

        let key_data = Self::unseal_with_pcr(sealed_blob, &entry.pcr_state);
        debug!(key_id, "Key unsealed successfully");
        Ok(key_data)
    }

    async fn get_attestation(&self) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        // In production: TPM2_Quote over PCR values.
        // Stub: BLAKE3 hash of current PCR state.
        let pcr_state = self.current_pcr_state.read().await;
        let quote = blake3::hash(&pcr_state).as_bytes().to_vec();
        debug!(quote_len = quote.len(), "TPM attestation quote generated (stub)");
        Ok(quote)
    }

    async fn list_sealed_keys(&self) -> Result<Vec<KeyInfo>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        let keys = self.sealed_keys.read().await;
        Ok(keys.values().map(|e| e.info.clone()).collect())
    }

    async fn remove_sealed_key(&self, key_id: &str) -> Result<(), VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        let mut keys = self.sealed_keys.write().await;
        if keys.remove(key_id).is_none() {
            return Err(VaultError::TpmError(format!(
                "Key not found: {}",
                key_id
            )));
        }

        info!(key_id, "Sealed key removed from TPM");
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tpm_config() -> TpmConfig {
        TpmConfig {
            device_path: "/dev/tpmrm0".into(),
            required: false,
            pcr_indices: vec![0, 7],
        }
    }

    #[tokio::test]
    async fn test_tpm_available() {
        let tpm = TpmManager::new(test_tpm_config());
        assert!(tpm.is_available().await);
    }

    #[tokio::test]
    async fn test_seal_unseal_roundtrip() {
        let tpm = TpmManager::new(test_tpm_config());
        let key_data = b"super secret master key material";

        let sealed = tpm.seal_key(key_data, "master-key-1").await.unwrap();
        assert_ne!(sealed, key_data.to_vec()); // sealed should differ

        let unsealed = tpm.unseal_key(&sealed, "master-key-1").await.unwrap();
        assert_eq!(unsealed, key_data.to_vec());
    }

    #[tokio::test]
    async fn test_pcr_mismatch_blocks_unseal() {
        let tpm = TpmManager::new(test_tpm_config());
        let key_data = b"key sealed before firmware update";

        let sealed = tpm.seal_key(key_data, "pcr-test-key").await.unwrap();

        // Simulate firmware update (PCR change)
        tpm.simulate_pcr_change().await;

        // Unseal should fail due to PCR mismatch
        let result = tpm.unseal_key(&sealed, "pcr-test-key").await;
        assert!(matches!(result, Err(VaultError::TpmPcrMismatch)));
    }

    #[tokio::test]
    async fn test_pcr_recovery_after_reset() {
        let tpm = TpmManager::new(test_tpm_config());
        let key_data = b"recoverable key";

        let sealed = tpm.seal_key(key_data, "recover-key").await.unwrap();

        // Change PCR, verify blocked
        tpm.simulate_pcr_change().await;
        assert!(tpm.unseal_key(&sealed, "recover-key").await.is_err());

        // Reset PCR, verify accessible again
        tpm.reset_pcr_state().await;
        let unsealed = tpm.unseal_key(&sealed, "recover-key").await.unwrap();
        assert_eq!(unsealed, key_data.to_vec());
    }

    #[tokio::test]
    async fn test_list_and_remove_keys() {
        let tpm = TpmManager::new(test_tpm_config());

        tpm.seal_key(b"key1", "id-1").await.unwrap();
        tpm.seal_key(b"key2", "id-2").await.unwrap();

        let keys = tpm.list_sealed_keys().await.unwrap();
        assert_eq!(keys.len(), 2);

        tpm.remove_sealed_key("id-1").await.unwrap();
        let keys = tpm.list_sealed_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "id-2");
    }

    #[tokio::test]
    async fn test_attestation_quote() {
        let tpm = TpmManager::new(test_tpm_config());
        let quote = tpm.get_attestation().await.unwrap();
        assert_eq!(quote.len(), 32); // BLAKE3 hash size
    }
}
