//! Broker error type.

/// Failures from the STS credential broker.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    /// The presented trust token failed signature / revocation verification.
    #[error("trust token rejected: {0}")]
    TokenRejected(String),
    /// The presented trust token is expired.
    #[error("trust token expired")]
    TokenExpired,
    /// An OpenBao interaction failed (root-credential custody).
    #[error("openbao: {0}")]
    OpenBao(#[from] wsf_bridge::OpenBaoError),
    /// Root credentials were missing or malformed in OpenBao.
    #[error("root credential: {0}")]
    RootCredential(String),
    /// The resolved grant is unusable (e.g. its TTL ceiling is below the STS
    /// floor) — refuse rather than widen.
    #[error("grant rejected: {0}")]
    Grant(String),
    /// STS transport failure.
    #[error("sts transport: {0}")]
    Http(#[from] reqwest::Error),
    /// STS returned an error response.
    #[error("sts error: {0}")]
    Sts(String),
    /// The STS response could not be parsed.
    #[error("sts response parse: {0}")]
    Parse(String),
}
