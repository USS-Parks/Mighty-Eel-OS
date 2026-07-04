//! Post-quantum cryptography engine.
//!
//! Implements `PqcProvider` using ML-KEM-1024 (FIPS 203) for key encapsulation
//! and ML-DSA-87 (FIPS 204) for digital signatures. Bulk model-weight data is
//! encrypted with AES-256-GCM under a key derived from the KEM shared secret
//! via HKDF-SHA3-256.
//!
//! # Cryptographic backend
//!
//! Pure-Rust RustCrypto `ml-kem` (FIPS 203) + `ml-dsa` (FIPS 204), enabled by
//! the default `pqc-dev` feature. No C dependencies; builds on every platform
//! and runs offline / air-gapped. The `ml-dsa` crate is pre-1.0 but functionally
//! correct and not flagged by `cargo audit`; it is also fronted by the
//! `fabric-crypto` signer abstraction for the Sovereignty Stack.
//!
//! The former `pqc-prod` liboqs backend (via the now-archived `pqcrypto-*`
//! crates) was removed in SOV-0.2b — nothing in any ship profile selected it. A
//! FIPS-validated liboqs path can return later behind a new feature using the
//! maintained `oqs` crate, if an ITAR/defense deployment requires it.
//!
//! The trait surface exposes only `Vec<u8>` blobs, so callers never depend on
//! the backend.
//!
//! # Key Hierarchy
//!
//! ```text
//! master_signing_key (ML-DSA-87, generated at initialize())
//!   -> per-model KEM keypair (ML-KEM-1024, generated lazily on first encrypt)
//!       -> AES-256-GCM session key (derived per-encryption via HKDF)
//! ```
//!
//! # Envelope format
//!
//! Encrypted model weights use the following on-disk layout:
//!
//! ```text
//! [ kem_ciphertext (1568 B) | nonce (12 B) | aead_ciphertext + tag (N+16 B) ]
//! ```
//!
//! The receiver looks up the per-model ML-KEM secret key by `model_id`,
//! decapsulates to recover the shared secret, derives the AES key via HKDF,
//! and decrypts the trailing AEAD payload.

use std::collections::HashMap;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use hkdf::Hkdf;
use rand::RngCore;
use sha3::Sha3_256;
use tokio::sync::RwLock;
use tracing::{debug, info};

use mai_core::vault::{KeyInfo, KeyLevel, PqcProvider, VaultError};

use crate::config::PqcConfig;

// ---------------------------------------------------------------------------
// Constants (FIPS 203 / 204 sizes)
// ---------------------------------------------------------------------------

/// ML-KEM-1024 public key size in bytes (FIPS 203).
const MLKEM1024_PK_LEN: usize = 1568;
/// ML-KEM-1024 secret key size in bytes (FIPS 203).
const MLKEM1024_SK_LEN: usize = 3168;
/// ML-KEM-1024 ciphertext size in bytes (FIPS 203).
const MLKEM1024_CT_LEN: usize = 1568;
/// ML-KEM shared secret size in bytes.
const MLKEM_SS_LEN: usize = 32;

/// ML-DSA-87 public key size in bytes (FIPS 204).
const MLDSA87_PK_LEN: usize = 2592;
/// ML-DSA-87 secret/signing key size in bytes (FIPS 204).
const MLDSA87_SK_LEN: usize = 4896;
/// ML-DSA-87 signature size in bytes (FIPS 204).
const MLDSA87_SIG_LEN: usize = 4627;

/// AES-256-GCM nonce length.
const AES_NONCE_LEN: usize = 12;
/// AES-256-GCM key length.
const AES_KEY_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Feature-gated KEM backend
// ---------------------------------------------------------------------------

#[cfg(feature = "pqc-dev")]
mod kem_backend {
    use super::{MLKEM1024_PK_LEN, MLKEM1024_SK_LEN, VaultError};
    use ml_kem::kem::{Decapsulate, Encapsulate};
    use ml_kem::{Encoded, EncodedSizeUser, KemCore, MlKem1024};

    pub fn keypair() -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let mut rng = rand::thread_rng();
        let (dk, ek) = MlKem1024::generate(&mut rng);
        let pk = ek.as_bytes().to_vec();
        let sk = dk.as_bytes().to_vec();
        debug_assert_eq!(pk.len(), MLKEM1024_PK_LEN);
        debug_assert_eq!(sk.len(), MLKEM1024_SK_LEN);
        Ok((pk, sk))
    }

    pub fn encapsulate(public_key: &[u8]) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let arr = Encoded::<<MlKem1024 as KemCore>::EncapsulationKey>::try_from(public_key)
            .map_err(|_| VaultError::PqcError("ML-KEM-1024 public key decode failed".into()))?;
        let ek = <MlKem1024 as KemCore>::EncapsulationKey::from_bytes(&arr);
        let mut rng = rand::thread_rng();
        let (ct, ss) = ek
            .encapsulate(&mut rng)
            .map_err(|_| VaultError::PqcError("ML-KEM-1024 encapsulation failed".into()))?;
        Ok((ct.to_vec(), ss.to_vec()))
    }

    pub fn decapsulate(ciphertext: &[u8], secret_key: &[u8]) -> Result<Vec<u8>, VaultError> {
        let dk_arr = Encoded::<<MlKem1024 as KemCore>::DecapsulationKey>::try_from(secret_key)
            .map_err(|_| VaultError::PqcError("ML-KEM-1024 secret key decode failed".into()))?;
        let dk = <MlKem1024 as KemCore>::DecapsulationKey::from_bytes(&dk_arr);
        let ct_arr = ml_kem::Ciphertext::<MlKem1024>::try_from(ciphertext)
            .map_err(|_| VaultError::PqcError("ML-KEM-1024 ciphertext decode failed".into()))?;
        let ss = dk
            .decapsulate(&ct_arr)
            .map_err(|_| VaultError::PqcError("ML-KEM-1024 decapsulation failed".into()))?;
        Ok(ss.to_vec())
    }
}

// ---------------------------------------------------------------------------
// DSA backend (pure-Rust ML-DSA-87)
// ---------------------------------------------------------------------------

#[cfg(feature = "pqc-dev")]
mod dsa_backend {
    use super::{MLDSA87_PK_LEN, MLDSA87_SIG_LEN, MLDSA87_SK_LEN, VaultError};
    use ml_dsa::signature::{Signer, Verifier};
    use ml_dsa::{B32, EncodedSignature, KeyGen, MlDsa87, Signature, SigningKey, VerifyingKey};

    pub fn keypair() -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let mut seed_bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut seed_bytes);
        let seed = B32::from(seed_bytes);
        let kp = MlDsa87::key_gen_internal(&seed);
        let sk = kp.signing_key().encode().to_vec();
        let pk = kp.verifying_key().encode().to_vec();
        debug_assert_eq!(pk.len(), MLDSA87_PK_LEN);
        debug_assert_eq!(sk.len(), MLDSA87_SK_LEN);
        Ok((pk, sk))
    }

    pub fn sign(data: &[u8], signing_key: &[u8]) -> Result<Vec<u8>, VaultError> {
        let sk_arr: &[u8; MLDSA87_SK_LEN] = signing_key
            .try_into()
            .map_err(|_| VaultError::PqcError("ML-DSA-87 sk wrong size".into()))?;
        let sk_encoded = ml_dsa::EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(data);
        let bytes = sig.encode().to_vec();
        debug_assert_eq!(bytes.len(), MLDSA87_SIG_LEN);
        Ok(bytes)
    }

    pub fn verify(data: &[u8], signature: &[u8], public_key: &[u8]) -> Result<bool, VaultError> {
        if signature.len() != MLDSA87_SIG_LEN {
            return Ok(false);
        }
        let pk_arr: &[u8; MLDSA87_PK_LEN] = public_key
            .try_into()
            .map_err(|_| VaultError::PqcError("ML-DSA-87 pk wrong size".into()))?;
        let sig_arr: &[u8; MLDSA87_SIG_LEN] = signature
            .try_into()
            .map_err(|_| VaultError::PqcError("ML-DSA-87 sig wrong size".into()))?;
        let pk_encoded = ml_dsa::EncodedVerifyingKey::<MlDsa87>::from(*pk_arr);
        let pk = VerifyingKey::<MlDsa87>::decode(&pk_encoded);
        let sig_encoded = EncodedSignature::<MlDsa87>::from(*sig_arr);
        let sig = match Signature::<MlDsa87>::decode(&sig_encoded) {
            Some(s) => s,
            None => return Ok(false),
        };
        Ok(pk.verify(data, &sig).is_ok())
    }
}

// Compile-time guard: the pure-Rust PQC backend must be selected. The former
// `pqc-prod` liboqs backend (archived `pqcrypto-*`) was removed in SOV-0.2b.
#[cfg(not(feature = "pqc-dev"))]
compile_error!("mai-vault requires the `pqc-dev` PQC backend feature.");

// ---------------------------------------------------------------------------
// PqcEngine
// ---------------------------------------------------------------------------

/// Post-quantum cryptography engine.
pub struct PqcEngine {
    config: PqcConfig,
    /// Per-model KEM keypairs: key_id -> (public, secret, metadata).
    keys: RwLock<HashMap<String, KeyPair>>,
    /// Model-to-key mapping.
    model_keys: RwLock<HashMap<String, String>>,
    /// Master ML-DSA signing keypair (initialised by `initialize()`).
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

    /// Initialise the engine: generate the master ML-DSA-87 signing keypair.
    ///
    /// Idempotent within a single process; calling twice replaces the keypair
    /// (used by tests; production code should call once at first boot).
    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            kem = %self.config.kem_algorithm,
            dsa = %self.config.dsa_algorithm,
            "Initialising PQC engine"
        );
        let (pub_key, sign_key) = dsa_backend::keypair()?;
        let mut sk = self.signing_key.write().await;
        *sk = Some(DsaKeyPair {
            public_key: pub_key,
            signing_key: sign_key,
        });
        info!("PQC engine initialised with ML-DSA-87 signing keypair");
        Ok(())
    }

    /// Return the master ML-DSA-87 public key.
    pub async fn signing_public_key(&self) -> Result<Vec<u8>, VaultError> {
        let sk = self.signing_key.read().await;
        match sk.as_ref() {
            Some(kp) => Ok(kp.public_key.clone()),
            None => Err(VaultError::PqcError(
                "Signing keypair not initialised".into(),
            )),
        }
    }

    /// List all managed per-model KEM keys.
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

    /// Get-or-create the KEM keypair bound to `model_id`.
    ///
    /// Returns `(public_key, secret_key)`. Generates a fresh keypair on first
    /// call for a given `model_id`.
    async fn ensure_model_keypair(&self, model_id: &str) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        {
            let model_keys = self.model_keys.read().await;
            if let Some(key_id) = model_keys.get(model_id) {
                let keys = self.keys.read().await;
                if let Some(kp) = keys.get(key_id) {
                    return Ok((kp.public_key.clone(), kp.secret_key.clone()));
                }
            }
        }
        let (pk, sk) = kem_backend::keypair()?;
        let key_id = format!("model-key-{model_id}");
        #[allow(clippy::cast_sign_loss)]
        let created_at = chrono::Utc::now().timestamp() as u64;
        {
            let mut keys = self.keys.write().await;
            keys.insert(
                key_id.clone(),
                KeyPair {
                    key_id: key_id.clone(),
                    public_key: pk.clone(),
                    secret_key: sk.clone(),
                    level: KeyLevel::ModelEncryption,
                    created_at,
                    model_id: Some(model_id.to_string()),
                },
            );
        }
        {
            let mut model_keys = self.model_keys.write().await;
            model_keys.insert(model_id.to_string(), key_id);
        }
        Ok((pk, sk))
    }

    /// Derive a 32-byte AES key from a KEM shared secret using HKDF-SHA3-256.
    fn derive_aes_key(shared_secret: &[u8], salt: &[u8], info: &[u8]) -> [u8; AES_KEY_LEN] {
        let hk = Hkdf::<Sha3_256>::new(Some(salt), shared_secret);
        let mut okm = [0u8; AES_KEY_LEN];
        hk.expand(info, &mut okm)
            .expect("HKDF expansion of 32 bytes from SHA3-256 cannot fail");
        okm
    }
}

// ---------------------------------------------------------------------------
// PqcProvider implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PqcProvider for PqcEngine {
    async fn kem_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let (pk, sk) = kem_backend::keypair()?;
        debug!(
            pk_len = pk.len(),
            sk_len = sk.len(),
            "Generated ML-KEM-1024 keypair"
        );
        Ok((pk, sk))
    }

    async fn kem_encapsulate(&self, public_key: &[u8]) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        if public_key.len() != MLKEM1024_PK_LEN {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-KEM-1024 public key length: {} (expected {MLKEM1024_PK_LEN})",
                public_key.len()
            )));
        }
        let (ct, ss) = kem_backend::encapsulate(public_key)?;
        debug!("ML-KEM-1024 encapsulation complete");
        Ok((ct, ss))
    }

    async fn kem_decapsulate(
        &self,
        ciphertext: &[u8],
        secret_key: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        if secret_key.len() != MLKEM1024_SK_LEN {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-KEM-1024 secret key length: {} (expected {MLKEM1024_SK_LEN})",
                secret_key.len()
            )));
        }
        if ciphertext.len() != MLKEM1024_CT_LEN {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-KEM-1024 ciphertext length: {} (expected {MLKEM1024_CT_LEN})",
                ciphertext.len()
            )));
        }
        let ss = kem_backend::decapsulate(ciphertext, secret_key)?;
        debug!("ML-KEM-1024 decapsulation complete");
        Ok(ss)
    }

    async fn dsa_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
        let (pk, sk) = dsa_backend::keypair()?;
        debug!(
            pk_len = pk.len(),
            sk_len = sk.len(),
            "Generated ML-DSA-87 keypair"
        );
        Ok((pk, sk))
    }

    async fn dsa_sign(&self, data: &[u8], signing_key: &[u8]) -> Result<Vec<u8>, VaultError> {
        if signing_key.len() != MLDSA87_SK_LEN {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-DSA-87 signing key length: {} (expected {MLDSA87_SK_LEN})",
                signing_key.len()
            )));
        }
        let sig = dsa_backend::sign(data, signing_key)?;
        debug!(data_len = data.len(), "ML-DSA-87 signature generated");
        Ok(sig)
    }

    async fn dsa_verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<bool, VaultError> {
        if public_key.len() != MLDSA87_PK_LEN {
            return Err(VaultError::PqcError(format!(
                "Invalid ML-DSA-87 public key length: {} (expected {MLDSA87_PK_LEN})",
                public_key.len()
            )));
        }
        let valid = dsa_backend::verify(data, signature, public_key)?;
        debug!(data_len = data.len(), valid, "ML-DSA-87 verify result");
        Ok(valid)
    }

    async fn encrypt_model_weights(
        &self,
        model_id: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        debug!(
            model_id,
            bytes = plaintext.len(),
            "Encrypting model weights"
        );
        let (pk, _sk) = self.ensure_model_keypair(model_id).await?;
        let (kem_ct, shared_secret) = kem_backend::encapsulate(&pk)?;
        let aes_key = Self::derive_aes_key(
            &shared_secret,
            model_id.as_bytes(),
            b"mai-vault/model-weights/v1",
        );
        let cipher = Aes256Gcm::new_from_slice(&aes_key)
            .map_err(|e| VaultError::PqcError(format!("AES-256-GCM init: {e}")))?;
        let mut nonce_bytes = [0u8; AES_NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let aead_ct = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| VaultError::PqcError(format!("AES-256-GCM encrypt: {e}")))?;
        let mut envelope = Vec::with_capacity(kem_ct.len() + AES_NONCE_LEN + aead_ct.len());
        envelope.extend_from_slice(&kem_ct);
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&aead_ct);
        info!(
            model_id,
            bytes = envelope.len(),
            "Model weights encrypted (ML-KEM-1024 + AES-256-GCM)"
        );
        Ok(envelope)
    }

    async fn decrypt_model_weights(
        &self,
        model_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        debug!(
            model_id,
            bytes = ciphertext.len(),
            "Decrypting model weights"
        );
        if ciphertext.len() < MLKEM1024_CT_LEN + AES_NONCE_LEN + 16 {
            return Err(VaultError::PqcError(
                "Ciphertext envelope too short for ML-KEM + AES-GCM payload".into(),
            ));
        }
        let kem_ct = &ciphertext[..MLKEM1024_CT_LEN];
        let nonce_bytes = &ciphertext[MLKEM1024_CT_LEN..MLKEM1024_CT_LEN + AES_NONCE_LEN];
        let aead_ct = &ciphertext[MLKEM1024_CT_LEN + AES_NONCE_LEN..];

        let model_keys = self.model_keys.read().await;
        let key_id = model_keys.get(model_id).ok_or_else(|| {
            VaultError::PqcError(format!("No KEM keypair registered for model {model_id}"))
        })?;
        let keys = self.keys.read().await;
        let kp = keys.get(key_id).ok_or_else(|| {
            VaultError::PqcError(format!("KEM keypair {key_id} missing from store"))
        })?;

        let shared_secret = kem_backend::decapsulate(kem_ct, &kp.secret_key)?;
        let aes_key = Self::derive_aes_key(
            &shared_secret,
            model_id.as_bytes(),
            b"mai-vault/model-weights/v1",
        );
        let cipher = Aes256Gcm::new_from_slice(&aes_key)
            .map_err(|e| VaultError::PqcError(format!("AES-256-GCM init: {e}")))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce_bytes), aead_ct)
            .map_err(|_| VaultError::PqcError("AES-256-GCM authentication failed".into()))?;
        info!(model_id, bytes = plaintext.len(), "Model weights decrypted");
        Ok(plaintext)
    }

    async fn sign_package(&self, package_data: &[u8]) -> Result<Vec<u8>, VaultError> {
        let sk = self.signing_key.read().await;
        let kp = sk
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("Signing keypair not initialised".into()))?;
        dsa_backend::sign(package_data, &kp.signing_key)
    }

    async fn verify_package(
        &self,
        package_data: &[u8],
        signature: &[u8],
    ) -> Result<bool, VaultError> {
        let sk = self.signing_key.read().await;
        let kp = sk
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("Signing keypair not initialised".into()))?;
        dsa_backend::verify(package_data, signature, &kp.public_key)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(pk.len(), MLKEM1024_PK_LEN);
        assert_eq!(sk.len(), MLKEM1024_SK_LEN);
    }

    #[tokio::test]
    async fn test_kem_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.kem_generate_keypair().await.unwrap();
        let (ct, ss1) = engine.kem_encapsulate(&pk).await.unwrap();
        assert_eq!(ct.len(), MLKEM1024_CT_LEN);
        assert_eq!(ss1.len(), MLKEM_SS_LEN);
        let ss2 = engine.kem_decapsulate(&ct, &sk).await.unwrap();
        assert_eq!(ss1, ss2);
    }

    #[tokio::test]
    async fn test_kem_decap_wrong_key_diverges() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk_a, _sk_a) = engine.kem_generate_keypair().await.unwrap();
        let (_pk_b, sk_b) = engine.kem_generate_keypair().await.unwrap();
        let (ct, ss_a) = engine.kem_encapsulate(&pk_a).await.unwrap();
        // ML-KEM implicit-rejection: decapsulating with a foreign key returns
        // a deterministic pseudo-random secret, not an error. The two shared
        // secrets must not match.
        let ss_b = engine.kem_decapsulate(&ct, &sk_b).await.unwrap();
        assert_ne!(ss_a, ss_b);
    }

    #[tokio::test]
    async fn test_dsa_keypair_sizes() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();
        assert_eq!(pk.len(), MLDSA87_PK_LEN);
        assert_eq!(sk.len(), MLDSA87_SK_LEN);
    }

    #[tokio::test]
    async fn test_dsa_sign_verify_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();
        let data = b"test message for ML-DSA-87 signing";
        let signature = engine.dsa_sign(data, &sk).await.unwrap();
        assert_eq!(signature.len(), MLDSA87_SIG_LEN);
        assert!(engine.dsa_verify(data, &signature, &pk).await.unwrap());
    }

    #[tokio::test]
    async fn test_dsa_tamper_detection() {
        let engine = PqcEngine::new(test_pqc_config());
        let (pk, sk) = engine.dsa_generate_keypair().await.unwrap();
        let signature = engine.dsa_sign(b"original message", &sk).await.unwrap();
        assert!(
            !engine
                .dsa_verify(b"tampered message", &signature, &pk)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_encrypt_decrypt_roundtrip() {
        let engine = PqcEngine::new(test_pqc_config());
        let plaintext = b"model weight data for AEAD round-trip test".to_vec();
        let encrypted = engine
            .encrypt_model_weights("test-model-rt", &plaintext)
            .await
            .unwrap();
        assert_ne!(encrypted, plaintext);
        assert!(encrypted.len() >= MLKEM1024_CT_LEN + AES_NONCE_LEN + 16);
        let decrypted = engine
            .decrypt_model_weights("test-model-rt", &encrypted)
            .await
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[tokio::test]
    async fn test_aead_tamper_detection() {
        let engine = PqcEngine::new(test_pqc_config());
        let mut ct = engine
            .encrypt_model_weights("tamper-model", b"important data")
            .await
            .unwrap();
        // Flip a byte in the AEAD payload (after KEM ciphertext + nonce).
        let flip_at = MLKEM1024_CT_LEN + AES_NONCE_LEN + 1;
        ct[flip_at] ^= 0x01;
        let result = engine.decrypt_model_weights("tamper-model", &ct).await;
        assert!(result.is_err(), "tampered ciphertext must fail AEAD verify");
    }

    #[tokio::test]
    async fn test_package_sign_verify() {
        let engine = PqcEngine::new(test_pqc_config());
        engine.initialize().await.unwrap();
        let package = b"model package binary data";
        let sig = engine.sign_package(package).await.unwrap();
        assert!(engine.verify_package(package, &sig).await.unwrap());
    }

    #[tokio::test]
    async fn test_signing_key_required_for_package_ops() {
        let engine = PqcEngine::new(test_pqc_config());
        // No initialize() call.
        assert!(engine.sign_package(b"data").await.is_err());
        assert!(
            engine
                .verify_package(b"data", &[0u8; MLDSA87_SIG_LEN])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_invalid_key_sizes_rejected() {
        let engine = PqcEngine::new(test_pqc_config());
        assert!(engine.kem_encapsulate(&[0u8; 100]).await.is_err());
        assert!(
            engine
                .kem_decapsulate(&[0u8; MLKEM1024_CT_LEN], &[0u8; 100])
                .await
                .is_err()
        );
        assert!(engine.dsa_sign(b"test", &[0u8; 100]).await.is_err());
        assert!(
            engine
                .dsa_verify(b"test", &[0u8; MLDSA87_SIG_LEN], &[0u8; 100])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_per_model_key_isolation() {
        let engine = PqcEngine::new(test_pqc_config());
        let ct_a = engine
            .encrypt_model_weights("model-a", b"alpha")
            .await
            .unwrap();
        // Decrypting model-a's envelope under model-b's id must fail because
        // model-b's secret key cannot decapsulate model-a's KEM ciphertext.
        let _ = engine
            .encrypt_model_weights("model-b", b"beta")
            .await
            .unwrap();
        let result = engine.decrypt_model_weights("model-b", &ct_a).await;
        assert!(result.is_err());
    }
}
