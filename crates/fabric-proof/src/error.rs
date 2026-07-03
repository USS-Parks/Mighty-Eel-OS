//! Error type for `fabric-proof`.

/// A failure from an audit-proof primitive.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProofError {
    /// The per-tenant HMAC key is shorter than [`crate::MIN_TENANT_KEY_LEN`].
    #[error("tenant HMAC key too short: {got} bytes (minimum {min})")]
    TenantKeyTooShort {
        /// Length of the key that was supplied.
        got: usize,
        /// Minimum required key length.
        min: usize,
    },
    /// Canonical-JSON serialization of a value failed.
    #[error("canonical serialization failed: {0}")]
    Serialize(String),
    /// No trust anchor is registered for the requested `public_key_id`.
    #[error("no trust anchor registered for public_key_id {0:?}")]
    MissingTrustAnchor(String),
    /// The signature failed ML-DSA-87 verification (or was malformed).
    #[error("signature failed verification")]
    InvalidSignature,
    /// The registered trust-anchor public key has the wrong size.
    #[error("trust anchor public key is malformed")]
    InvalidPublicKey,
    /// A hex-encoded signature string was not valid lowercase hex.
    #[error("signature bytes are not valid hex")]
    MalformedSignatureHex,
    /// A hash chain link did not extend the prior link.
    #[error("hash chain broken at index {index}: {detail}")]
    ChainBroken {
        /// Index of the first link that failed to verify.
        index: usize,
        /// Human-readable reason.
        detail: String,
    },
}
