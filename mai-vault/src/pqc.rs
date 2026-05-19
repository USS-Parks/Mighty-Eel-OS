//! Post-quantum cryptography engine.
//!
//! Implements `PqcProvider` using ML-KEM-1024 (FIPS 203) for key encapsulation
//! and ML-DSA-87 (FIPS 204) for digital signatures. Both are NIST PQC standards
//! finalized in 2024.
//!
//! # Key Hierarchy
//!
//! ```text
//! master_key (TPM-sealed, ML-KEM-1024)
//!   -> model_encryption_key (per-model, derived via KEM)
//!     -> file_key (per-weight-file, derived via KEM)
//! ```
//!
//! # Bulk Encryption
//!
//! Model weights are encrypted with AES-256-GCM. The symmetric key is
//! wrapped via ML-KEM key encapsulation. This provides post-quantum
//! security for data at rest without the performance cost of encrypting
//! large files directly with lattice-based cryptography.
//!
//! # Stub Status
//!
//! This implementation provides structurally correct trait implementations
//! using deterministic test vectors. Real PQC operations require linking
//! against liboqs or the pqcrypto crate family at build time.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{KeyInfo, KeyLevel, PqcProvider, VaultError};

use crate::config::PqcConfig;

/// Post-quantum cryptography engine.
///
/// In production, delegates to liboqs or pqcrypto for actual ML-KEM/ML-DSA
/// operations. This implementation uses deterministic stubs that maintain
/// the correct data flow for integration testing.
pub struct PqcEngine {
    config: PqcConfig,
    /// Key store: key_id -> (public_key, secret_key)
    keys: RwLock<HashMap<String, KeyPair>>,
    /// Model-to-key mapping: model_id -> key_id
    model_keys: RwLock<HashMap<String, String>>,
    /// Signing keypair (DSA) for audit signatures
    signing_key: RwLock<Option<DsaKeyPair>>,
}

/// An ML-KEM keypair (encapsulation).
struct KeyPair {
    pub key_id: String,
    pub public_key: Vec<u8>,
    pub secret_key: Vec<u8>,
    pub level: KeyLevel,
    pub created_at: u64,
    pub model_id: Option<String>,
}

/// An ML-DSA keypair (signing).
struct DsaKeyPair {
    pub public_key: Vec<u8>,
    pub signing_key: Vec<u8>,
}

impl PqcEngine {
    /// Create a new PQC engine with the given configuration.
    pub fn new(config: PqcConfig) -> Self {
        Self {
            config,
            keys: RwLock::new(HashMap::new()),
            model_keys: RwLock::new(HashMap::new()),
            signing_key: RwLock::new(None),
        }
    }

    /// Initialize the engine: generate or load the master signing keypair.
    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            kem = %self.config.kem_algorithm,
            dsa = %self.config.dsa_algorithm,
            "Initializing PQC engine"
        );

        // Generate the master signing keypair (ML-DSA-87)
        let (pub_key, sign_key) = self.dsa_generate_keypair().await?;
        let mut sk = self.signing_key.write().await;
        *sk = Some(DsaKeyPair {
            public_key: pub_key,
            signing_key: sign_key,
        });

        info!("PQC engine initialized with ML-DSA signing keypair");
        Ok(())
    }

    /// Generate a deterministic test key (stub).
    ///
    /// In production, this calls `ml_kem_1024_keypair()` from liboqs.
    fn generate_stub_kem_keypair() -> (Vec<u8>, Vec<u8>) {
        // ML-KEM-1024 public key is 1568 bytes, secret key is 3168 bytes.
        // We generate deterministic stubs of the correct size.
        let public_key = vec![0xAA; 1568];
        let secret_key = vec![0xBB; 3168];
        (public_key, secret_key)
    }

    /// Generate a deterministic test DSA key (stub).
    ///
    /// In production, this calls `ml_dsa_87_keypair()` from liboqs.
    fn generate_stub_dsa_keypair() -> (Vec<u8>, Vec<u8>) {
        // ML-DSA-87 public key is 2592 bytes, signing key is 4896 bytes.
        let public_key = vec![0xCC; 2592];
        let signing_key = vec![0xDD; 4896];
        (public_key, signing_key)
    }

    /// Deterministic stub encryption (XOR with key prefix).
    ///
    /// In production: AES-256-GCM with key derived from ML-KEM shared secret.
    fn stub_encrypt(plaintext: &[u8], key_hint: &[u8]) -> Vec<u8> {
        let mut out = plaintext.to_vec();
        for (i, byte) in out.iter_mut().enumerate() {
            *byte ^= key_hint[i % key_hint.len().max(1)];
        }
        out
    }

    /// Deterministic stub decryption (same XOR operation).
    fn stub_decrypt(ciphertext: &[u8], key_hint: &[u8]) -> Vec<u8> {
        // XOR is its own inverse
        Self::stub_encrypt(ciphertext, key_hint)
    }

    /// Get the current signing public key.
    pub async fn signing_public_key(&self) -> Result<Vec<u8>, VaultError> {
        let sk = self.signing_key.read().await;
        match sk.as_ref() {
            Some(kp) => Ok(kp.public_key.clone()),
            None => Err(VaultError::PqcError(
                "Signing keypair not initialized".into(),
            )),
        }
    }

    /// List all managed keys.
    pub async fn list_keys(&self) -> Vec<KeyInfo> {
        let keys = self.keys.read().await;
        keys.values()
            .map(|kp| KeyInfo {
                key_id: kp.key_id.clone(),
                level: kp.level,
                algorithm: "ML-KEM-1024".to_string(),
                created_at: kp.created_at,
                model_id: kp.model_id.clone(),
                tpm_sealed: false,
            })
            .collect()
    }
}

#[async_trait]
impl PqcProvider for PqcEngine {
    async fn kem_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let (pk, sk) = Self::generate_stub_kem_keypair();
        debug!(
            pk_len = pk.len(),
            sk_len = sk.len(),
            "Generated ML-KEM-1024 keypair (stub)"
        );
        Ok((pk, sk))
    }

    async fn kem_encapsulate(
        &self,
        public_key: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        if public_key.len() != 1568 {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-KEM-1024 public key length: {} (expected 1568)",
                public_key.len()
            )));
        }
        // Stub: ciphertext is 1568 bytes, shared secret is 32 bytes
        let ciphertext = vec![0xEE; 1568];
        let shared_secret = vec![0xFF; 32];
        debug!("ML-KEM encapsulation complete (stub)");
        Ok((ciphertext, shared_secret))
    }

    async fn kem_decapsulate(
        &self,
        ciphertext: &[u8],
        secret_key: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        if secret_key.len() != 3168 {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-KEM-1024 secret key length: {} (expected 3168)",
                secret_key.len()
            )));
        }
        // Stub: return deterministic shared secret
        let shared_secret = vec![0xFF; 32];
        debug!("ML-KEM decapsulation complete (stub)");
        Ok(shared_secret)
    }

    async fn dsa_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let (pk, sk) = Self::generate_stub_dsa_keypair();
        debug!(
            pk_len = pk.len(),
            sk_len = sk.len(),
            "Generated ML-DSA-87 keypair (stub)"
        );
        Ok((pk, sk))
    }

    async fn dsa_sign(
        &self,
        data: &[u8],
        signing_key: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        if signing_key.len() != 4896 {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-DSA-87 signing key length: {} (expected 4896)",
                signing_key.len()
            )));
        }
        // Stub: BLAKE3 hash of data as "signature" (NOT cryptographically secure)
        let hash = blake3::hash(data);
        let mut signature = vec![0u8; 4627]; // ML-DSA-87 signature is 4627 bytes
        let hash_bytes = hash.as_bytes();
        signature[..32].copy_from_slice(hash_bytes);
        debug!(data_len = data.len(), "ML-DSA-87 signature generated (stub)");
        Ok(signature)
    }

    async fn dsa_verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<bool, VaultError> {
        if public_key.len() != 2592 {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-DSA-87 public key length: {} (expected 2592)",
                public_key.len()
            )));
        }
        // Stub: verify by recomputing BLAKE3 and comparing first 32 bytes
        let hash = blake3::hash(data);
        let hash_bytes = hash.as_bytes();
        let valid = signature.len() >= 32 && signature[..32] == hash_bytes[..];
        debug!(data_len = data.len(), valid, "ML-DSA-87 signature verified (stub)");
        Ok(valid)
    }

    async fn encrypt_model_weights(
        &self,
        model_id: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        // High-level flow:
        // 1. Look up (or generate) the model encryption key
        // 2. KEM-encapsulate to get symmetric key
        // 3. AES-256-GCM encrypt plaintext with symmetric key
        // 4. Prepend KEM ciphertext to encrypted data
        debug!(model_id, bytes = plaintext.len(), "Encrypting model weights");

        // Stub: XOR-based "encryption" for structural correctness
        let key_hint = model_id.as_bytes();
        let ciphertext = Self::stub_encrypt(plaintext, key_hint);

        info!(model_id, bytes = ciphertext.len(), "Model weights encrypted (stub)");
        Ok(ciphertext)
    }

    async fn decrypt_model_weights(
        &self,
        model_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        debug!(model_id, bytes = ciphertext.len(), "Decrypting model weights");

        let key_hint = model_id.as_bytes();
        let plaintext = Self::stub_decrypt(ciphertext, key_hint);

        info!(model_id, bytes = plaintext.len(), "Model weights decrypted (stub)");
        Ok(plaintext)
    }

    async fn sign_package(&self, package_data: &[u8]) -> Result<Vec<u8>, VaultError> {
        let sk = self.signing_key.read().await;
        let signing_key = sk
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("Signing keypair not initialized".into()))?;
        self.dsa_sign(package_data, &signing_key.signing_key).await
    }

    async fn verify_package(
        &self,
        package_data: &[u8],
        signature: &[u8],
    ) -> Result<bool, VaultError> {
        let sk = self.signing_key.read().await;
        let signing_key = sk
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("Signing keypair not initialized".into()))?;
        self.dsa_verify(package_data, signature, &signing_key.public_key)
            .await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PqcConfig;
    use std::path::PathBuf;

    fn test_pqc_config() -> PqcConfig {
        PqcConfig {
            kem_algorithm: "ML-KEM-1024".into(),
            dsa_algorithm: "ML-DSA-87".into(),
            key_store_path: PathBuf::from("/tmp/test-keys"),
            symmetric_cipher: "AES-256-GCM".into(),
        }
    }

    #[tokio::test]
    async fn test_kem_keypair_sizes() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.kem_generate_keypair().await.unwrap();
        assert_eq!(pk.len(), 1568); // ML-KEM-1024 public key
        assert_eq!(sk.len(), 3168); // ML-KEM-1024 secret key
    }

    #[tokio::test]
    async fn test_kem_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.kem_generate_keypair().await.unwrap();

        let (ct, ss1) = engine.kem_encapsulate(&pk).await.unwrap();
        let ss2 = engine.kem_decapsulate(&ct, &sk).await.unwrap();

        assert_eq!(ss1, ss2); // shared secrets must match
    }

    #[tokio::test]
    async fn test_dsa_keypair_sizes() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();
        assert_eq!(pk.len(), 2592); // ML-DSA-87 public key
        assert_eq!(sk.len(), 4896); // ML-DSA-87 signing key
    }

    #[tokio::test]
    async fn test_dsa_sign_verify_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();

        let data = b"test message for signing";
        let signature = engine.dsa_sign(data, &sk).await.unwrap();
        assert_eq!(signature.len(), 4627); // ML-DSA-87 signature size

        let valid = engine.dsa_verify(data, &signature, &pk).await.unwrap();
        assert!(valid);
    }

    #[tokio::test]
    async fn test_dsa_tamper_detection() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();

        let data = b"original message";
        let signature = engine.dsa_sign(data, &sk).await.unwrap();

        // Tampered data should fail verification
        let tampered = b"tampered message";
        let valid = engine.dsa_verify(tampered, &signature, &pk).await.unwrap();
        assert!(!valid);
    }

    #[tokio::test]
    async fn test_encrypt_decrypt_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());

        let plaintext = b"model weight data for encryption test";
        let encrypted = engine
            .encrypt_model_weights("test-model", plaintext)
            .await
            .unwrap();

        // Encrypted data should differ from plaintext
        assert_ne!(encrypted, plaintext.to_vec());

        let decrypted = engine
            .decrypt_model_weights("test-model", &encrypted)
            .await
            .unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[tokio::test]
    async fn test_package_sign_verify() {
        let engine = PqcEngine::new(test_pqc_config());
        engine.initialize().await.unwrap();

        let package = b"model package binary data";
        let sig = engine.sign_package(package).await.unwrap();
        let valid = engine.verify_package(package, &sig).await.unwrap();
        assert!(valid);
    }

    #[tokio::test]
    async fn test_invalid_key_sizes_rejected() {
        let engine = PqcEngine::new(test_pqc_config());

        // Wrong public key size for encapsulate
        let result = engine.kem_encapsulate(&[0u8; 100]).await;
        assert!(result.is_err());

        // Wrong secret key size for decapsulate
        let result = engine.kem_decapsulate(&[0u8; 1568], &[0u8; 100]).await;
        assert!(result.is_err());

        // Wrong signing key size
        let result = engine.dsa_sign(b"test", &[0u8; 100]).await;
        assert!(result.is_err());

        // Wrong public key size for verify
        let result = engine.dsa_verify(b"test", &[0u8; 4627], &[0u8; 100]).await;
        assert!(result.is_err());
    }
}
