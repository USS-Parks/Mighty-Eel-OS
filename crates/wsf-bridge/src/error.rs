//! Trust Bridge error type.

use crate::openbao::OpenBaoError;

/// Failures from Trust Bridge operations.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// An OpenBao interaction (auth event or tenant read) failed.
    #[error(transparent)]
    OpenBao(#[from] OpenBaoError),
    /// Token issuance (canonicalize / sign) failed.
    #[error("token issuance failed: {0}")]
    Token(#[from] fabric_token::TokenError),
    /// A cryptographic operation failed.
    #[error("crypto failure: {0}")]
    Crypto(#[from] fabric_crypto::CryptoError),
    /// Revocation-snapshot signing failed.
    #[error("revocation signing failed: {0}")]
    Revocation(#[from] fabric_revocation::RevocationError),
    /// Subject pseudonymization (HMAC) failed.
    #[error("subject hash failed: {0}")]
    SubjectHash(#[from] fabric_proof::ProofError),
    /// A tenant attribute could not be mapped to a contract enum, or the
    /// configuration was invalid.
    #[error("configuration: {0}")]
    Config(String),
}
