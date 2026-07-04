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
//! # Backends
//!
//! * **Software TPM (default)** — `seal_key` encrypts the input with
//!   XChaCha20-Poly1305 under an HKDF-SHA3-256 key derived from the current
//!   PCR state. PCR mismatch causes AEAD verification to fail on unseal, so
//!   the binding is cryptographic rather than a plaintext equality check.
//! * **Hardware TPM** — gated behind the `tpm-hardware` feature flag and
//!   only available on Linux (via the `tss-esapi` crate at `/dev/tpmrm0`).
//!   Default builds do not require a TPM device.

use std::collections::HashMap;

use async_trait::async_trait;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha3::Sha3_256;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{KeyInfo, KeyLevel, TpmProvider, VaultError};

use crate::config::TpmConfig;

/// XChaCha20-Poly1305 key length in bytes.
const TPM_AEAD_KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 nonce length in bytes.
const TPM_AEAD_NONCE_LEN: usize = 24;
/// HKDF info tag identifying the sealing-key derivation domain.
const TPM_SEAL_INFO: &[u8] = b"mai-vault/tpm-seal/v1";

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

/// A sealed key entry in the software TPM.
///
/// Sealed bytes are stored as `[24-byte nonce | XChaCha20-Poly1305 ct+tag]`.
/// The encryption key is derived from the current PCR state at unseal time,
/// so successful decryption proves PCR consistency without a separately
/// stored seal-time PCR snapshot.
struct SealedKeyEntry {
    /// The full sealed envelope (nonce || AEAD ciphertext+tag).
    sealed_blob: Vec<u8>,
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
        // Stub mode uses a fixed "healthy boot" state.
        blake3::hash(b"healthy-boot-pcr-0-7").as_bytes().to_vec()
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

    /// Derive the AEAD sealing key from a PCR state via HKDF-SHA3-256.
    fn derive_seal_key(pcr_state: &[u8]) -> [u8; TPM_AEAD_KEY_LEN] {
        let hk = Hkdf::<Sha3_256>::new(None, pcr_state);
        let mut okm = [0u8; TPM_AEAD_KEY_LEN];
        hk.expand(TPM_SEAL_INFO, &mut okm)
            .expect("HKDF expansion of 32 bytes from SHA3-256 cannot fail");
        okm
    }

    /// Seal `key_data` with XChaCha20-Poly1305 under a key derived from
    /// `pcr_state`. Output envelope: `[24-byte nonce | ciphertext+tag]`.
    fn seal_with_pcr(key_data: &[u8], pcr_state: &[u8]) -> Result<Vec<u8>, VaultError> {
        let aead_key = Self::derive_seal_key(pcr_state);
        let cipher = XChaCha20Poly1305::new_from_slice(&aead_key)
            .map_err(|e| VaultError::TpmError(format!("XChaCha20-Poly1305 init: {e}")))?;
        let mut nonce_bytes = [0u8; TPM_AEAD_NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let aead_ct = cipher
            .encrypt(nonce, key_data)
            .map_err(|e| VaultError::TpmError(format!("TPM seal encrypt: {e}")))?;
        let mut envelope = Vec::with_capacity(TPM_AEAD_NONCE_LEN + aead_ct.len());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&aead_ct);
        Ok(envelope)
    }

    /// Unseal a sealed envelope using the current PCR state.
    ///
    /// Returns `TpmPcrMismatch` if the AEAD authentication tag is invalid,
    /// which is the cryptographic indicator that the PCR state has drifted
    /// from the seal-time state (or the ciphertext was tampered).
    fn unseal_with_pcr(sealed_data: &[u8], pcr_state: &[u8]) -> Result<Vec<u8>, VaultError> {
        if sealed_data.len() < TPM_AEAD_NONCE_LEN + 16 {
            return Err(VaultError::TpmError(
                "Sealed envelope too short for nonce+tag".into(),
            ));
        }
        let nonce_bytes = &sealed_data[..TPM_AEAD_NONCE_LEN];
        let aead_ct = &sealed_data[TPM_AEAD_NONCE_LEN..];
        let aead_key = Self::derive_seal_key(pcr_state);
        let cipher = XChaCha20Poly1305::new_from_slice(&aead_key)
            .map_err(|e| VaultError::TpmError(format!("XChaCha20-Poly1305 init: {e}")))?;
        cipher
            .decrypt(XNonce::from_slice(nonce_bytes), aead_ct)
            .map_err(|_| VaultError::TpmPcrMismatch)
    }
}

#[async_trait]
impl TpmProvider for TpmManager {
    async fn is_available(&self) -> bool {
        self.available
    }

    async fn seal_key(&self, key_data: &[u8], key_id: &str) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        let pcr_state = self.current_pcr_state.read().await.clone();
        let sealed_blob = Self::seal_with_pcr(key_data, &pcr_state)?;

        info!(
            key_id,
            blob_len = sealed_blob.len(),
            "Key sealed to TPM PCR state (XChaCha20-Poly1305)"
        );

        let mut keys = self.sealed_keys.write().await;
        keys.insert(
            key_id.to_string(),
            SealedKeyEntry {
                sealed_blob: sealed_blob.clone(),
                info: KeyInfo {
                    key_id: key_id.to_string(),
                    level: KeyLevel::Master,
                    algorithm: "TPM-SEAL/XChaCha20-Poly1305".to_string(),
                    #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
                    created_at: chrono::Utc::now().timestamp() as u64,
                    model_id: None,
                    tpm_sealed: true,
                },
            },
        );

        Ok(sealed_blob)
    }

    async fn unseal_key(&self, sealed_blob: &[u8], key_id: &str) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        // Look up the entry only to authorize that this key_id is known.
        // The actual ciphertext binding is enforced by AEAD authentication.
        let keys = self.sealed_keys.read().await;
        if !keys.contains_key(key_id) {
            return Err(VaultError::TpmError(format!("Key not found: {key_id}")));
        }
        drop(keys);

        let current_pcr = self.current_pcr_state.read().await.clone();
        let key_data = Self::unseal_with_pcr(sealed_blob, &current_pcr).map_err(|e| {
            if matches!(e, VaultError::TpmPcrMismatch) {
                warn!(
                    key_id,
                    "PCR state mismatch - system boot chain may have changed"
                );
            }
            e
        })?;
        debug!(key_id, "Key unsealed successfully");
        Ok(key_data)
    }

    async fn get_attestation(&self) -> Result<Vec<u8>, VaultError> {
        if !self.available {
            return Err(VaultError::TpmUnavailable);
        }

        // In production: TPM2_Quote over PCR values.
        // Stub mode hashes the current PCR state with BLAKE3.
        let pcr_state = self.current_pcr_state.read().await;
        let quote = blake3::hash(&pcr_state).as_bytes().to_vec();
        debug!(
            quote_len = quote.len(),
            "TPM attestation quote generated (stub)"
        );
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
            return Err(VaultError::TpmError(format!("Key not found: {key_id}")));
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
