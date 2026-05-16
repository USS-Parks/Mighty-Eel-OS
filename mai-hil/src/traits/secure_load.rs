use async_trait::async_trait;
use crate::HilError;

/// `SecureLoadContext`: Interface for TPM-attested, encrypted model loading.
/// Ensures model weights are never exposed in plaintext outside the secure boundary.
#[async_trait]
pub trait SecureLoadContext: Send + Sync {
    /// Unseals the vault master key from TPM 2.0.
    /// Fails if PCR registers do not match expected state.
    async fn unseal_tpm_key(&self) -> Result<Vec<u8>, HilError>;

    /// Decrypts model weights in-place using ML-KEM, verifying integrity via ML-DSA hash tree.
    async fn decrypt_and_verify(&self, encrypted_blob: &[u8], manifest_hash: &str) -> Result<Vec<u8>, HilError>;
}
