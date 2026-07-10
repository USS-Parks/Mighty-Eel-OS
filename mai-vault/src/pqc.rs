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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha3::Sha3_256;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{KeyInfo, KeyLevel, PqcProvider, TpmProvider, VaultError};

use crate::config::PqcConfig;

/// A per-model KEM key persisted to the key store (plan V9 — restart recovery).
/// The secret is wrapped (AES-256-GCM) under the store's key-encryption key, so
/// the file never holds a plaintext secret; the public key and metadata are
/// clear for indexing.
#[derive(Serialize, Deserialize)]
struct PersistedModelKey {
    key_id: String,
    model_id: String,
    public_key: Vec<u8>,
    /// `nonce(12) || AES-256-GCM ciphertext` of the KEM secret key.
    wrapped_secret: Vec<u8>,
    created_at: u64,
}

// ---------------------------------------------------------------------------
// Constants (FIPS 203 / 204 sizes)
// ---------------------------------------------------------------------------

/// ML-KEM-1024 public key size in bytes (FIPS 203).
const MLKEM1024_PK_LEN: usize = 1568;
/// ML-KEM-1024 secret key size in bytes (FIPS 203).
const MLKEM1024_SK_LEN: usize = 3168;
/// ML-KEM-1024 ciphertext size in bytes (FIPS 203).
const MLKEM1024_CT_LEN: usize = 1568;
/// ML-KEM shared secret size in bytes (FIPS 203). Referenced by the KEM tests.
#[allow(dead_code)]
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

/// Seal identifier for the key-store KEK (the vault's master key-encryption
/// key) when a TPM seal provider is wired.
pub const KEK_KEY_ID: &str = "vault-master-kek";
/// Legacy plaintext KEK file — the dev/no-seal form, migrated away from when a
/// seal provider is present.
const KEK_PLAINTEXT_FILE: &str = "kek.bin";
/// Sealed KEK envelope file — the only at-rest KEK form once a seal provider
/// is wired.
const KEK_SEALED_FILE: &str = "kek.sealed";

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
    /// Key-encryption key for wrapping persisted model secrets (plan V9).
    /// Loaded from / created in the key store on first use, so model keys —
    /// and therefore encrypted models — survive a restart.
    kek: RwLock<Option<[u8; 32]>>,
    /// TPM seal provider for the KEK. When wired, the KEK lives on disk
    /// only as a sealed envelope (`kek.sealed`) and loading it requires a
    /// successful unseal — PCR drift or an unavailable TPM fails closed. When
    /// absent (dev / stub posture), the legacy plaintext `kek.bin` is used.
    seal: RwLock<Option<Arc<dyn TpmProvider>>>,
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

/// A signer over a fixed 32-byte digest, backed by key material held inside the
/// implementor — the raw key bytes are never exposed to the caller. Lets the
/// vault hand out a signing capability (e.g. for the compliance audit chain)
/// without surrendering the master signing key.
pub trait DigestSigner: Send + Sync + std::fmt::Debug {
    /// Sign a 32-byte digest, returning the ML-DSA-87 signature bytes, or `None`
    /// if the held key is unusable.
    fn sign_digest(&self, digest: &[u8; 32]) -> Option<Vec<u8>>;
}

/// A [`DigestSigner`] backed by an ML-DSA-87 signing key. Holds the key bytes
/// internally; its `Debug` never renders them.
struct MasterKeyDigestSigner {
    signing_key: Vec<u8>,
}

impl std::fmt::Debug for MasterKeyDigestSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKeyDigestSigner")
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

impl DigestSigner for MasterKeyDigestSigner {
    fn sign_digest(&self, digest: &[u8; 32]) -> Option<Vec<u8>> {
        dsa_backend::sign(digest, &self.signing_key).ok()
    }
}

impl PqcEngine {
    /// Create a new PQC engine with the given configuration.
    pub fn new(config: PqcConfig) -> Self {
        Self {
            config,
            keys: RwLock::new(HashMap::new()),
            model_keys: RwLock::new(HashMap::new()),
            signing_key: RwLock::new(None),
            kek: RwLock::new(None),
            seal: RwLock::new(None),
        }
    }

    /// Wire a TPM seal provider for the key-store KEK. Must be set before
    /// [`initialize`](Self::initialize) (or the first key operation) so the
    /// KEK is created — or migrated from the legacy plaintext file — in sealed
    /// form. Once set, a KEK that cannot be sealed or unsealed is a hard
    /// error: the engine fails closed rather than falling back to plaintext.
    pub async fn set_seal_provider(&self, provider: Arc<dyn TpmProvider>) {
        *self.seal.write().await = Some(provider);
    }

    /// Directory holding persisted model keys.
    fn model_keys_dir(&self) -> PathBuf {
        self.config.key_store_path.join("model-keys")
    }

    fn model_key_file(&self, key_id: &str) -> PathBuf {
        self.model_keys_dir().join(format!("{key_id}.json"))
    }

    /// The key-store KEK — the root of restart recovery (plan V9): the same
    /// KEK unwraps the persisted model secrets after a reboot.
    ///
    /// With a seal provider wired, the KEK lives on disk only as the
    /// sealed envelope `kek.sealed` (a legacy plaintext `kek.bin` is migrated
    /// into it and removed), and a failed seal or unseal is a hard error.
    /// Without one (dev / stub posture), the legacy plaintext `kek.bin` on
    /// the (still-encrypted) dataset is used, as before.
    async fn kek(&self) -> Result<[u8; 32], VaultError> {
        if let Some(k) = *self.kek.read().await {
            return Ok(k);
        }
        let mut slot = self.kek.write().await;
        if let Some(k) = *slot {
            return Ok(k);
        }
        let seal = self.seal.read().await.clone();
        let kek = match seal {
            Some(tpm) => self.load_or_create_sealed_kek(tpm.as_ref()).await?,
            None => self.load_or_create_plaintext_kek()?,
        };
        *slot = Some(kek);
        Ok(kek)
    }

    /// Sealed-KEK path: unseal `kek.sealed` through the provider, or
    /// migrate a legacy plaintext `kek.bin` into a sealed envelope (deleting
    /// the plaintext), or generate a fresh KEK sealed-at-birth. The plaintext
    /// form never persists once a seal provider is wired; any seal/unseal
    /// failure (TPM unavailable, PCR drift, tampered envelope) fails closed.
    async fn load_or_create_sealed_kek(
        &self,
        tpm: &dyn TpmProvider,
    ) -> Result<[u8; 32], VaultError> {
        let sealed_path = self.config.key_store_path.join(KEK_SEALED_FILE);
        let plain_path = self.config.key_store_path.join(KEK_PLAINTEXT_FILE);
        if sealed_path.exists() {
            let envelope =
                std::fs::read(&sealed_path).map_err(|e| VaultError::IoError(e.to_string()))?;
            let bytes = tpm.unseal_key(&envelope, KEK_KEY_ID).await?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| VaultError::PqcError("unsealed KEK is not 32 bytes".into()))?;
            return Ok(arr);
        }
        let kek: [u8; 32] = if plain_path.exists() {
            // Migrate the legacy plaintext KEK: same value (so existing
            // wrapped model keys stay decryptable), now sealed at rest.
            let bytes =
                std::fs::read(&plain_path).map_err(|e| VaultError::IoError(e.to_string()))?;
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| VaultError::PqcError("kek.bin is not 32 bytes".into()))?
        } else {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let envelope = tpm.seal_key(&kek, KEK_KEY_ID).await?;
        std::fs::create_dir_all(&self.config.key_store_path)
            .map_err(|e| VaultError::IoError(e.to_string()))?;
        std::fs::write(&sealed_path, &envelope).map_err(|e| VaultError::IoError(e.to_string()))?;
        if plain_path.exists() {
            std::fs::remove_file(&plain_path).map_err(|e| VaultError::IoError(e.to_string()))?;
            info!("legacy plaintext KEK migrated into a TPM-sealed envelope");
        }
        Ok(kek)
    }

    /// Legacy plaintext-KEK path (dev / no seal provider): unchanged behavior.
    fn load_or_create_plaintext_kek(&self) -> Result<[u8; 32], VaultError> {
        let path = self.config.key_store_path.join(KEK_PLAINTEXT_FILE);
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| VaultError::IoError(e.to_string()))?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| VaultError::PqcError("kek.bin is not 32 bytes".into()))?;
            Ok(arr)
        } else {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            std::fs::create_dir_all(&self.config.key_store_path)
                .map_err(|e| VaultError::IoError(e.to_string()))?;
            std::fs::write(&path, k).map_err(|e| VaultError::IoError(e.to_string()))?;
            Ok(k)
        }
    }

    /// Wrap a model secret under the KEK and write it to the key store.
    async fn persist_model_key(&self, kp: &KeyPair) -> Result<(), VaultError> {
        let kek = self.kek().await?;
        let cipher = Aes256Gcm::new_from_slice(&kek)
            .map_err(|e| VaultError::PqcError(format!("KEK cipher init: {e}")))?;
        let mut nonce_bytes = [0u8; AES_NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), kp.secret_key.as_slice())
            .map_err(|e| VaultError::PqcError(format!("wrap model key: {e}")))?;
        let mut wrapped = Vec::with_capacity(AES_NONCE_LEN + ct.len());
        wrapped.extend_from_slice(&nonce_bytes);
        wrapped.extend_from_slice(&ct);
        let record = PersistedModelKey {
            key_id: kp.key_id.clone(),
            model_id: kp.model_id.clone().unwrap_or_default(),
            public_key: kp.public_key.clone(),
            wrapped_secret: wrapped,
            created_at: kp.created_at,
        };
        let json = serde_json::to_vec(&record).map_err(|e| VaultError::IoError(e.to_string()))?;
        std::fs::create_dir_all(self.model_keys_dir())
            .map_err(|e| VaultError::IoError(e.to_string()))?;
        std::fs::write(self.model_key_file(&kp.key_id), json)
            .map_err(|e| VaultError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Load a persisted model key into the in-memory maps, if one exists on
    /// disk (plan V9 restart recovery). Returns whether a key was loaded.
    async fn load_persisted_model_key(&self, model_id: &str) -> Result<bool, VaultError> {
        let key_id = format!("model-key-{model_id}");
        let file = self.model_key_file(&key_id);
        if !file.exists() {
            return Ok(false);
        }
        let bytes = std::fs::read(&file).map_err(|e| VaultError::IoError(e.to_string()))?;
        let record: PersistedModelKey =
            serde_json::from_slice(&bytes).map_err(|e| VaultError::IoError(e.to_string()))?;
        if record.wrapped_secret.len() < AES_NONCE_LEN + 16 {
            return Err(VaultError::PqcError(
                "persisted key wrapper too short".into(),
            ));
        }
        let kek = self.kek().await?;
        let cipher = Aes256Gcm::new_from_slice(&kek)
            .map_err(|e| VaultError::PqcError(format!("KEK cipher init: {e}")))?;
        let (nonce, ct) = record.wrapped_secret.split_at(AES_NONCE_LEN);
        let secret = cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| VaultError::PqcError("KEK unwrap failed (wrong key store?)".into()))?;
        {
            let mut keys = self.keys.write().await;
            keys.insert(
                record.key_id.clone(),
                KeyPair {
                    key_id: record.key_id.clone(),
                    public_key: record.public_key,
                    secret_key: secret,
                    level: KeyLevel::ModelEncryption,
                    created_at: record.created_at,
                    model_id: Some(model_id.to_string()),
                },
            );
        }
        self.model_keys
            .write()
            .await
            .insert(model_id.to_string(), record.key_id);
        Ok(true)
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
        {
            let mut sk = self.signing_key.write().await;
            *sk = Some(DsaKeyPair {
                public_key: pub_key,
                signing_key: sign_key,
            });
        }
        // Materialize the key-store KEK at boot, not lazily at the first
        // model install — so a wired seal provider seals it (or fails closed)
        // on the boot path, and readiness can runtime-probe the sealed
        // envelope immediately.
        self.kek().await?;
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

    /// A [`DigestSigner`] backed by this engine's master signing key, paired with
    /// the public key to register with a verifier. The signer holds the key
    /// material internally; raw secret bytes are never returned. This gives the
    /// compliance audit chain a real signer without exporting the key across the
    /// vault boundary. `None` until the engine is initialised.
    pub async fn audit_chain_signer(&self) -> Option<(Vec<u8>, std::sync::Arc<dyn DigestSigner>)> {
        let sk = self.signing_key.read().await;
        let kp = sk.as_ref()?;
        let signer: std::sync::Arc<dyn DigestSigner> = std::sync::Arc::new(MasterKeyDigestSigner {
            signing_key: kp.signing_key.clone(),
        });
        Some((kp.public_key.clone(), signer))
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
        // V9: recover a key persisted by a prior process before generating a
        // new one — so re-sealing the same model reuses its key and weights
        // sealed before a restart stay decryptable.
        if self.load_persisted_model_key(model_id).await? {
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
        let kp = KeyPair {
            key_id: key_id.clone(),
            public_key: pk.clone(),
            secret_key: sk.clone(),
            level: KeyLevel::ModelEncryption,
            created_at,
            model_id: Some(model_id.to_string()),
        };
        // Persist before publishing in memory: a key we cannot durably store
        // would silently fail to survive a restart, so a persist failure is an
        // error, not a warning.
        self.persist_model_key(&kp).await?;
        self.keys.write().await.insert(key_id.clone(), kp);
        self.model_keys
            .write()
            .await
            .insert(model_id.to_string(), key_id);
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

/// Runtime seal assertion for the vault master KEK: proves — by
/// measurement, never by config flag — that the key store's KEK is sealed.
///
/// Passes only when ALL of: the seal provider reports available, the sealed
/// envelope `kek.sealed` exists, it unseals under the **current** PCR state to
/// a well-formed 32-byte KEK, and no plaintext `kek.bin` lingers beside it.
/// Whether the provider is hardware-backed is the hardware-lane deferral; this
/// probe asserts what the configured provider actually protects on this host.
///
/// Returns `Ok(detail)` on a proven seal, `Err(reason)` otherwise — the caller
/// maps these onto the production-readiness runtime check.
pub async fn probe_sealed_master_key(
    key_store_path: &Path,
    tpm: &dyn TpmProvider,
) -> Result<String, String> {
    if !tpm.is_available().await {
        return Err("TPM seal provider unavailable — the master KEK cannot be sealed".to_string());
    }
    let sealed_path = key_store_path.join(KEK_SEALED_FILE);
    let plain_path = key_store_path.join(KEK_PLAINTEXT_FILE);
    if !sealed_path.exists() {
        return Err(format!(
            "no sealed master KEK at {} — the vault booted without sealing its master key",
            sealed_path.display()
        ));
    }
    if plain_path.exists() {
        return Err(format!(
            "plaintext KEK {} persists beside the sealed envelope — sealing is not the \
             at-rest form of the master key",
            plain_path.display()
        ));
    }
    let envelope = std::fs::read(&sealed_path)
        .map_err(|e| format!("sealed KEK {} unreadable: {e}", sealed_path.display()))?;
    let bytes = tpm.unseal_key(&envelope, KEK_KEY_ID).await.map_err(|e| {
        format!("sealed master KEK does not unseal under the current PCR state: {e}")
    })?;
    if bytes.len() != 32 {
        return Err("sealed master KEK unseals to malformed key material".to_string());
    }
    Ok(
        "master KEK sealed: kek.sealed unseals under the current PCR state; no plaintext KEK \
         on disk"
            .to_string(),
    )
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

        // V9: recover the key from the store if this process hasn't loaded it
        // yet (e.g. first decrypt after a restart).
        if !self.model_keys.read().await.contains_key(model_id) {
            self.load_persisted_model_key(model_id).await?;
        }

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

    async fn crypto_erase_model(&self, model_id: &str) -> Result<bool, VaultError> {
        // V7: retire the model's KEM key. Once the secret is gone, the
        // ML-KEM-1024 + AES-256-GCM envelope on disk (and in every ZFS
        // snapshot that retains it) can never be decapsulated again — the
        // deletion is cryptographic, independent of copy-on-write block
        // retention. The key id is deterministic, so erasure works whether or
        // not this process has loaded the key into memory (V9: a key persisted
        // by a prior boot is still retired).
        let key_id = format!("model-key-{model_id}");
        let mut retired = false;

        self.model_keys.write().await.remove(model_id);
        if let Some(mut kp) = self.keys.write().await.remove(&key_id) {
            kp.secret_key.iter_mut().for_each(|b| *b = 0); // scrub before free
            retired = true;
        }

        // Delete the persisted (KEK-wrapped) key so a restart cannot resurrect
        // it. Absence is fine (already erased); any other IO error is real.
        let file = self.model_key_file(&key_id);
        match std::fs::remove_file(&file) {
            Ok(()) => retired = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!(model_id, error = %e, "failed to delete persisted model key");
                return Err(VaultError::IoError(e.to_string()));
            }
        }

        if retired {
            info!(
                model_id,
                key_id, "Model encryption key retired (crypto-erase)"
            );
        }
        Ok(retired)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pqc_config() -> PqcConfig {
        // A unique key store per call: model keys now persist to disk (V9), so
        // a shared fixed path would leak state across tests.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mai-pqc-test-{}-{n}", std::process::id()));
        PqcConfig {
            kem_algorithm: "ML-KEM-1024".into(),
            dsa_algorithm: "ML-DSA-87".into(),
            key_store_path: dir,
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
    async fn audit_chain_signer_signs_verifiably() {
        // The vault hands out an opaque digest signer (Option A); its signature
        // over a digest verifies under the returned public key — so the audit
        // chain gets a real, verifiable signer without the key leaving the vault.
        let engine = PqcEngine::new(test_pqc_config());
        engine.initialize().await.unwrap();
        let (pubkey, signer) = engine
            .audit_chain_signer()
            .await
            .expect("signer available after initialize");
        let digest = [7u8; 32];
        let sig = signer.sign_digest(&digest).expect("digest signs");
        assert!(
            dsa_backend::verify(&digest, &sig, &pubkey).unwrap(),
            "the audit signature verifies under the returned public key"
        );
        // Fail-closed before initialise: no signer is available.
        let fresh = PqcEngine::new(test_pqc_config());
        assert!(fresh.audit_chain_signer().await.is_none());
    }

    #[tokio::test]
    async fn v9_model_key_survives_restart() {
        // Seal weights with one engine, drop it (simulating a process exit),
        // then bring a fresh engine up over the SAME key store: it recovers
        // the persisted, KEK-wrapped model key and decrypts the weights that
        // were sealed before the "restart".
        let cfg = test_pqc_config();
        let plaintext = b"weights that must survive a reboot";
        let ciphertext = {
            let engine = PqcEngine::new(cfg.clone());
            engine
                .encrypt_model_weights("restart-model", plaintext)
                .await
                .unwrap()
        }; // engine dropped: in-memory keys gone, only the key store remains.

        let reborn = PqcEngine::new(cfg.clone());
        let recovered = reborn
            .decrypt_model_weights("restart-model", &ciphertext)
            .await
            .expect("a fresh engine must recover the persisted model key");
        assert_eq!(recovered, plaintext);
    }

    #[tokio::test]
    async fn v9_crypto_erase_survives_restart() {
        // After crypto-erasing a model, a fresh engine over the same key store
        // must NOT be able to decrypt retained ciphertext — the persisted key
        // is gone, not just the in-memory copy.
        let cfg = test_pqc_config();
        let ciphertext = {
            let engine = PqcEngine::new(cfg.clone());
            let ct = engine
                .encrypt_model_weights("erase-model", b"secret")
                .await
                .unwrap();
            assert!(engine.crypto_erase_model("erase-model").await.unwrap());
            ct
        };
        let reborn = PqcEngine::new(cfg.clone());
        let err = reborn
            .decrypt_model_weights("erase-model", &ciphertext)
            .await
            .unwrap_err();
        assert!(
            matches!(err, VaultError::PqcError(_)),
            "erased key must not resurrect after restart, got {err:?}"
        );
        // Erasing an already-absent model is a no-op that reports nothing done.
        assert!(!reborn.crypto_erase_model("erase-model").await.unwrap());
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

    // ---- sealed master KEK ------------------------------------------------

    use crate::config::TpmConfig;
    use crate::tpm::TpmManager;

    fn seal_tpm() -> Arc<TpmManager> {
        Arc::new(TpmManager::new(TpmConfig {
            device_path: "/dev/tpmrm0".into(),
            required: false,
            pcr_indices: vec![0, 7],
        }))
    }

    /// A seal backend that is simply not there — the "no-seal backend" case.
    #[derive(Debug)]
    struct NoSealBackend;

    #[async_trait]
    impl TpmProvider for NoSealBackend {
        async fn is_available(&self) -> bool {
            false
        }
        async fn seal_key(&self, _: &[u8], _: &str) -> Result<Vec<u8>, VaultError> {
            Err(VaultError::TpmUnavailable)
        }
        async fn unseal_key(&self, _: &[u8], _: &str) -> Result<Vec<u8>, VaultError> {
            Err(VaultError::TpmUnavailable)
        }
        async fn get_attestation(&self) -> Result<Vec<u8>, VaultError> {
            Err(VaultError::TpmUnavailable)
        }
        async fn list_sealed_keys(&self) -> Result<Vec<KeyInfo>, VaultError> {
            Err(VaultError::TpmUnavailable)
        }
        async fn remove_sealed_key(&self, _: &str) -> Result<(), VaultError> {
            Err(VaultError::TpmUnavailable)
        }
    }

    #[tokio::test]
    async fn sealed_kek_created_at_boot_and_survives_restart() {
        // With a seal provider wired, initialize() materializes the KEK as a
        // sealed envelope only — no plaintext form — and a later boot (fresh
        // engine, fresh TpmManager, same store) unseals it and decrypts
        // weights sealed before the restart.
        let cfg = test_pqc_config();
        let ciphertext = {
            let engine = PqcEngine::new(cfg.clone());
            engine.set_seal_provider(seal_tpm()).await;
            engine.initialize().await.unwrap();
            assert!(
                cfg.key_store_path.join("kek.sealed").exists(),
                "boot seals the KEK on the boot path"
            );
            assert!(
                !cfg.key_store_path.join("kek.bin").exists(),
                "no plaintext KEK is ever written when a seal provider is wired"
            );
            engine
                .encrypt_model_weights("sealed-model", b"sealed weights")
                .await
                .unwrap()
        };

        let reborn = PqcEngine::new(cfg.clone());
        reborn.set_seal_provider(seal_tpm()).await;
        let recovered = reborn
            .decrypt_model_weights("sealed-model", &ciphertext)
            .await
            .expect("a fresh boot unseals the KEK and recovers the model key");
        assert_eq!(recovered, b"sealed weights");
    }

    #[tokio::test]
    async fn legacy_plaintext_kek_migrates_into_sealed_envelope() {
        // A store initialized in the plaintext posture migrates: same KEK
        // value (old wrapped model keys stay decryptable), sealed at rest,
        // plaintext removed.
        let cfg = test_pqc_config();
        let ciphertext = {
            let plain = PqcEngine::new(cfg.clone());
            plain
                .encrypt_model_weights("migrate-model", b"pre-seal weights")
                .await
                .unwrap()
        };
        assert!(cfg.key_store_path.join("kek.bin").exists());

        let sealed = PqcEngine::new(cfg.clone());
        sealed.set_seal_provider(seal_tpm()).await;
        sealed.initialize().await.unwrap();
        assert!(
            cfg.key_store_path.join("kek.sealed").exists(),
            "migration seals the KEK"
        );
        assert!(
            !cfg.key_store_path.join("kek.bin").exists(),
            "migration removes the plaintext KEK"
        );
        let recovered = sealed
            .decrypt_model_weights("migrate-model", &ciphertext)
            .await
            .expect("the migrated KEK still unwraps pre-migration model keys");
        assert_eq!(recovered, b"pre-seal weights");
    }

    #[tokio::test]
    async fn probe_asserts_the_measured_seal_state() {
        let cfg = test_pqc_config();
        let tpm = seal_tpm();

        // No sealed envelope yet: the probe refuses.
        let err = probe_sealed_master_key(&cfg.key_store_path, tpm.as_ref())
            .await
            .unwrap_err();
        assert!(err.contains("no sealed master KEK"), "got: {err}");

        // Boot with sealing wired: the probe passes on measurement.
        let engine = PqcEngine::new(cfg.clone());
        engine.set_seal_provider(tpm.clone()).await;
        engine.initialize().await.unwrap();
        let detail = probe_sealed_master_key(&cfg.key_store_path, tpm.as_ref())
            .await
            .expect("sealed store probes clean");
        assert!(detail.contains("unseals under the current PCR state"));

        // Plaintext residue beside the envelope: fail closed.
        std::fs::write(cfg.key_store_path.join("kek.bin"), [0u8; 32]).unwrap();
        let err = probe_sealed_master_key(&cfg.key_store_path, tpm.as_ref())
            .await
            .unwrap_err();
        assert!(err.contains("plaintext KEK"), "got: {err}");
        std::fs::remove_file(cfg.key_store_path.join("kek.bin")).unwrap();

        // PCR drift (firmware change): the envelope no longer unseals — the
        // probe reports the runtime truth instead of a config flag.
        tpm.simulate_pcr_change().await;
        let err = probe_sealed_master_key(&cfg.key_store_path, tpm.as_ref())
            .await
            .unwrap_err();
        assert!(
            err.contains("does not unseal under the current PCR state"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn no_seal_backend_fails_closed() {
        // On a no-seal backend, both the boot path and the probe
        // refuse — never a silent plaintext fallback.
        let cfg = test_pqc_config();
        let engine = PqcEngine::new(cfg.clone());
        engine.set_seal_provider(Arc::new(NoSealBackend)).await;
        let err = engine.initialize().await.unwrap_err();
        assert!(
            matches!(err, VaultError::TpmUnavailable),
            "boot fails closed without a seal backend, got {err:?}"
        );
        assert!(
            !cfg.key_store_path.join("kek.bin").exists(),
            "fail-closed boot must not fall back to a plaintext KEK"
        );

        let err = probe_sealed_master_key(&cfg.key_store_path, &NoSealBackend)
            .await
            .unwrap_err();
        assert!(err.contains("unavailable"), "got: {err}");
    }
}
