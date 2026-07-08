//! SHIP-05: AEAD sealer for the compliance audit WAL.
//!
//! Replaces the bring-up [`NullSealer`] in production: every audit
//! entry is encrypted at rest with AES-256-GCM and a fresh 96-bit
//! nonce. Output format on disk is `nonce (12B) || ciphertext || tag`.
//!
//! Key acquisition is intentionally out of scope here. SHIP-05 ships
//! the primitive plus an explicit-key constructor; SHIP-07 wiring
//! folds the key under the vault-managed key path.
//!
//! [`NullSealer`]: super::store::NullSealer

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use rand::rngs::OsRng;

use super::store::{StoreError, StoreSealer};

/// Length of the sealed-record nonce prefix (96 bits, per AES-GCM).
pub const AEAD_SEALER_NONCE_LEN: usize = 12;

/// AES-256-GCM key length in bytes.
pub const AEAD_SEALER_KEY_LEN: usize = 32;

/// Errors produced when seeding an [`AeadSealer`] from external state.
#[derive(Debug, thiserror::Error)]
pub enum AeadSealerError {
    /// Caller passed a key whose byte count is not [`AEAD_SEALER_KEY_LEN`].
    #[error("AEAD sealer key must be exactly {expected} bytes; got {actual}")]
    InvalidKeyLength {
        /// Required length.
        expected: usize,
        /// Length the caller supplied.
        actual: usize,
    },
}

/// AES-256-GCM audit-store sealer.
pub struct AeadSealer {
    cipher: Aes256Gcm,
}

impl AeadSealer {
    /// Build a sealer from an explicit 32-byte key. SHIP-07 convergence
    /// loads the key out of the vault; until then callers supply it
    /// directly.
    pub fn new(key_bytes: &[u8; AEAD_SEALER_KEY_LEN]) -> Self {
        let key = Key::<Aes256Gcm>::from_slice(key_bytes);
        Self {
            cipher: Aes256Gcm::new(key),
        }
    }

    /// Build a sealer from a slice. Returns an error on the wrong length.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, AeadSealerError> {
        let fixed: &[u8; AEAD_SEALER_KEY_LEN] =
            bytes
                .try_into()
                .map_err(|_| AeadSealerError::InvalidKeyLength {
                    expected: AEAD_SEALER_KEY_LEN,
                    actual: bytes.len(),
                })?;
        Ok(Self::new(fixed))
    }

    /// Build a sealer with an OS-RNG-derived key. Suitable for tests
    /// and for the local-dev path when `allow_null_sealer=false`. The
    /// key is held only in memory and never persisted by this struct.
    pub fn with_ephemeral_key() -> Self {
        let mut key = [0u8; AEAD_SEALER_KEY_LEN];
        OsRng.fill_bytes(&mut key);
        Self::new(&key)
    }

    /// Decrypt a sealed record produced by this sealer. Provided for
    /// tests and the SHIP-07 audit reader; not part of the
    /// [`StoreSealer`] write-only contract.
    pub fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, aes_gcm::Error> {
        if sealed.len() < AEAD_SEALER_NONCE_LEN {
            return Err(aes_gcm::Error);
        }
        let (nonce_bytes, ciphertext) = sealed.split_at(AEAD_SEALER_NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher.decrypt(nonce, ciphertext)
    }
}

impl fmt::Debug for AeadSealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never expose key material — even indirectly — through Debug.
        f.debug_struct("AeadSealer").finish_non_exhaustive()
    }
}

impl StoreSealer for AeadSealer {
    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError> {
        // Delegate to the inherent decrypt; map the opaque AEAD failure to the
        // store's fail-closed error (wrong key or tampered ciphertext).
        AeadSealer::unseal(self, sealed).map_err(|_| StoreError::WalUnseal)
    }

    fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut nonce_bytes = [0u8; AEAD_SEALER_NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        // AES-GCM encrypt only fails on internal buffer issues, never
        // on user input. The trait is infallible, so a panic here is a
        // programming error in the aead crate, not in callers.
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext)
            .expect("AES-256-GCM encrypt is infallible for valid inputs");
        let mut out = Vec::with_capacity(AEAD_SEALER_NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_key_round_trip() {
        let key = [0x42u8; AEAD_SEALER_KEY_LEN];
        let sealer = AeadSealer::new(&key);
        let plaintext = b"audit entry payload";
        let sealed = sealer.seal(plaintext);
        assert!(
            sealed.len() > AEAD_SEALER_NONCE_LEN + plaintext.len(),
            "sealed output must include nonce + tag overhead"
        );
        let recovered = sealer.unseal(&sealed).expect("unseal matching key");
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn nonces_vary_per_call() {
        let sealer = AeadSealer::with_ephemeral_key();
        let pt = b"same plaintext";
        let a = sealer.seal(pt);
        let b = sealer.seal(pt);
        // Same plaintext, fresh nonces => different sealed records.
        assert_ne!(a, b, "AEAD must not produce identical sealed records");
        assert_ne!(&a[..AEAD_SEALER_NONCE_LEN], &b[..AEAD_SEALER_NONCE_LEN]);
    }

    #[test]
    fn wrong_key_fails_to_unseal() {
        let pt = b"secret";
        let sealed = AeadSealer::new(&[1u8; AEAD_SEALER_KEY_LEN]).seal(pt);
        let other = AeadSealer::new(&[2u8; AEAD_SEALER_KEY_LEN]);
        assert!(other.unseal(&sealed).is_err());
    }

    #[test]
    fn from_slice_rejects_wrong_length() {
        let err = AeadSealer::from_slice(&[0u8; 16]).expect_err("must reject short key");
        assert!(matches!(
            err,
            AeadSealerError::InvalidKeyLength {
                expected: 32,
                actual: 16,
            }
        ));
    }

    #[test]
    fn debug_does_not_leak_key() {
        let sealer = AeadSealer::new(&[0xABu8; AEAD_SEALER_KEY_LEN]);
        let dbg = format!("{sealer:?}");
        assert_eq!(dbg, "AeadSealer { .. }");
    }
}
