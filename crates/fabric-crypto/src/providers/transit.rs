//! Custody provider: OpenBao Transit ML-DSA signing. The signing key never
//! leaves the vault — the end-state the Trust Manifold (§8.1) points at.
//!
//! **Not yet available.** Open-source OpenBao Transit does not expose ML-DSA;
//! only Vault Enterprise 1.19 does, experimentally, and depending on Vault
//! Enterprise contradicts the sovereignty thesis. This provider is the seam that
//! lights up in Phase W when OSS Transit ships GA post-quantum signing. Until
//! then it fails closed, and appliances sign locally via
//! [`super::RustCryptoMlDsa87`].

use crate::Signer;
use crate::error::CryptoError;

/// A signer that delegates to an OpenBao Transit key. The endpoint, key name,
/// and auth token are wired in Phase W; today every operation fails closed.
pub struct TransitSigner {
    key_name: String,
}

impl TransitSigner {
    /// Name a Transit signing key. Constructing is cheap and always succeeds;
    /// signing is what fails closed until the backend is available.
    pub fn new(key_name: impl Into<String>) -> Self {
        Self {
            key_name: key_name.into(),
        }
    }
}

impl Signer for TransitSigner {
    fn algorithm(&self) -> &'static str {
        "ml-dsa-87"
    }

    fn key_id(&self) -> &str {
        &self.key_name
    }

    fn public_key(&self) -> &[u8] {
        &[]
    }

    fn sign(&self, _message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Err(CryptoError::Unavailable(
            "OpenBao Transit ML-DSA signing is not yet GA in open-source OpenBao; \
             use RustCryptoMlDsa87. This provider lights up in Phase W."
                .into(),
        ))
    }
}
