//! ML-DSA-87 signed-bundle verification over a 32-byte payload hash, backed by
//! [`fabric_crypto`]. A verifier holds an in-memory trust-anchor registry
//! (`public_key_id -> public key bytes`) populated at boot from config or vault.

use std::collections::BTreeMap;

use fabric_crypto::providers::MlDsa87Verifier;
use fabric_crypto::{MLDSA87_PK_LEN, Verifier};

use crate::error::ProofError;

/// Verify a signature over a `payload_hash` under the key named `public_key_id`.
pub trait BundleVerifier {
    /// # Errors
    /// Returns a [`ProofError`] if the anchor is unknown, the key is malformed,
    /// or the signature does not verify.
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), ProofError>;
}

/// ML-DSA-87-backed [`BundleVerifier`] with a named trust-anchor registry.
#[derive(Debug, Clone, Default)]
pub struct MlDsaBundleVerifier {
    anchors: BTreeMap<String, Vec<u8>>,
}

impl MlDsaBundleVerifier {
    /// Construct an empty verifier. Add anchors with [`Self::with_anchor`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a trust-anchor public key, returning `self` for chaining.
    #[must_use]
    pub fn with_anchor(mut self, public_key_id: impl Into<String>, public_key: Vec<u8>) -> Self {
        self.anchors.insert(public_key_id.into(), public_key);
        self
    }

    /// Number of registered trust anchors.
    #[must_use]
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }
}

impl BundleVerifier for MlDsaBundleVerifier {
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), ProofError> {
        let pk = self
            .anchors
            .get(public_key_id)
            .ok_or_else(|| ProofError::MissingTrustAnchor(public_key_id.to_string()))?;
        if pk.len() != MLDSA87_PK_LEN {
            return Err(ProofError::InvalidPublicKey);
        }
        match MlDsa87Verifier.verify(payload_hash, signature_bytes, pk) {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(ProofError::InvalidSignature),
        }
    }
}

/// Bring-up / test helper: accepts every signature. Never wire into a real
/// path — no runtime guard currently rejects it, so callers must not select it
/// outside tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptAllBundleVerifier;

impl BundleVerifier for AcceptAllBundleVerifier {
    fn verify(&self, _: &[u8; 32], _: &[u8], _: &str) -> Result<(), ProofError> {
        Ok(())
    }
}
