//! Default provider: RustCrypto `ml-dsa` (FIPS 204, ML-DSA-87). Pure Rust, no C
//! deps — a byte-for-byte mirror of mai-vault's proven `pqc-dev` `dsa_backend`,
//! so this introduces no new crypto library. This is the signer every appliance
//! carries for offline / air-gapped operation, where a networked custody backend
//! is unreachable.

use ml_dsa::signature::{Signer as MlDsaSigner, Verifier as MlDsaVerifier};
use ml_dsa::{
    B32, EncodedSignature, EncodedSigningKey, EncodedVerifyingKey, KeyGen, MlDsa87, Signature,
    SigningKey, VerifyingKey,
};

use zeroize::Zeroize;

use crate::error::CryptoError;
use crate::{MLDSA87_PK_LEN, MLDSA87_SIG_LEN, MLDSA87_SK_LEN, Signer, Verifier};

/// An ML-DSA-87 keypair-backed signer.
pub struct RustCryptoMlDsa87 {
    key_id: String,
    public_key: Vec<u8>,
    secret_key: Vec<u8>,
}

impl RustCryptoMlDsa87 {
    /// Generate a fresh ML-DSA-87 keypair and return the encoded `(public,
    /// secret)` bytes. Callers that manage their own key storage (e.g. the vault)
    /// use this, then reconstruct a signer with [`Self::from_keypair`].
    ///
    /// # Errors
    /// Infallible today; returns `Result` for forward compatibility.
    pub fn keypair() -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
        let mut seed_bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut seed_bytes);
        let seed = B32::from(seed_bytes);
        let kp = MlDsa87::key_gen_internal(&seed);
        let secret_key = kp.signing_key().encode().to_vec();
        let public_key = kp.verifying_key().encode().to_vec();
        // Wipe the KDF seed now that the keypair is derived — the seed alone
        // reconstructs the whole secret key. Clears the buffer we own; the
        // transient copy inside `key_gen_internal` is owned by ml-dsa.
        seed_bytes.zeroize();
        Ok((public_key, secret_key))
    }

    /// Generate a fresh keypair and wrap it in a signer under `key_id`.
    ///
    /// # Errors
    /// Returns [`CryptoError`] if key generation produces malformed material.
    pub fn generate(key_id: impl Into<String>) -> Result<Self, CryptoError> {
        let (public_key, secret_key) = Self::keypair()?;
        Self::from_keypair(key_id, public_key, secret_key)
    }

    /// Reconstruct a signer from previously generated encoded key material.
    ///
    /// # Errors
    /// Returns [`CryptoError::KeySize`] if `public_key` or `secret_key` is not
    /// the ML-DSA-87 encoded length.
    pub fn from_keypair(
        key_id: impl Into<String>,
        public_key: Vec<u8>,
        secret_key: Vec<u8>,
    ) -> Result<Self, CryptoError> {
        if public_key.len() != MLDSA87_PK_LEN {
            return Err(CryptoError::KeySize(format!(
                "ML-DSA-87 public key {} != {MLDSA87_PK_LEN}",
                public_key.len()
            )));
        }
        if secret_key.len() != MLDSA87_SK_LEN {
            return Err(CryptoError::KeySize(format!(
                "ML-DSA-87 secret key {} != {MLDSA87_SK_LEN}",
                secret_key.len()
            )));
        }
        Ok(Self {
            key_id: key_id.into(),
            public_key,
            secret_key,
        })
    }
}

impl Signer for RustCryptoMlDsa87 {
    fn algorithm(&self) -> &'static str {
        "ml-dsa-87"
    }

    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let sk_arr: &[u8; MLDSA87_SK_LEN] = self
            .secret_key
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::KeySize("ML-DSA-87 secret key wrong size".into()))?;
        let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(message);
        Ok(sig.encode().to_vec())
    }
}

impl Drop for RustCryptoMlDsa87 {
    /// Wipe the secret key from memory on drop (K3). Only the secret is cleared;
    /// `key_id` and `public_key` are not sensitive. Best-effort: guards the key
    /// against lingering in a freed heap buffer, not against copies ml-dsa made
    /// internally while signing.
    fn drop(&mut self) {
        self.secret_key.zeroize();
    }
}

/// Stateless ML-DSA-87 verifier — the public key is supplied per call.
pub struct MlDsa87Verifier;

impl Verifier for MlDsa87Verifier {
    fn algorithm(&self) -> &'static str {
        "ml-dsa-87"
    }

    fn verify(
        &self,
        message: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<bool, CryptoError> {
        if signature.len() != MLDSA87_SIG_LEN || public_key.len() != MLDSA87_PK_LEN {
            return Ok(false);
        }
        let pk_arr: &[u8; MLDSA87_PK_LEN] = public_key
            .try_into()
            .map_err(|_| CryptoError::KeySize("ML-DSA-87 public key wrong size".into()))?;
        let sig_arr: &[u8; MLDSA87_SIG_LEN] = signature
            .try_into()
            .map_err(|_| CryptoError::KeySize("ML-DSA-87 signature wrong size".into()))?;
        let pk = VerifyingKey::<MlDsa87>::decode(&EncodedVerifyingKey::<MlDsa87>::from(*pk_arr));
        let sig = match Signature::<MlDsa87>::decode(&EncodedSignature::<MlDsa87>::from(*sig_arr)) {
            Some(s) => s,
            None => return Ok(false),
        };
        Ok(pk.verify(message, &sig).is_ok())
    }
}
