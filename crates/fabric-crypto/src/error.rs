//! Error type for `fabric-crypto`.

/// A failure from a signer or verifier provider.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Key or signature material was the wrong size.
    #[error("crypto key size error: {0}")]
    KeySize(String),
    /// A signing operation failed.
    #[error("crypto signing error: {0}")]
    Sign(String),
    /// The provider is not available in this build or deployment.
    #[error("crypto provider unavailable: {0}")]
    Unavailable(String),
}
