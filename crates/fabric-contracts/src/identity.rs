//! Identity contract — see `contracts/identity.md`.

use serde::{Deserialize, Serialize};

use crate::common::Signature;

/// The server-created authenticated principal for a privileged WSF request.
///
/// A `WsfAuthenticator` produces this from a **verified** identity source (mTLS /
/// a signed workload-identity assertion) — never from ordinary request JSON — and
/// it is the sole authority for *who* a token is minted for. Token issuance copies
/// tenant / subject / roles from here; the request body may only *narrow* (model
/// subset, budget below the policy ceiling), never self-assign identity or roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsfPrincipal {
    /// Tenant the principal belongs to.
    pub tenant_id: String,
    /// Cleartext subject id (the bridge pseudonymizes it into `subject_hash`).
    pub subject_id: String,
    /// Optional workload / service identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_identity: Option<String>,
    /// Roles the principal is authorized for — from server-side policy, never
    /// caller-set.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Intended audience of tokens minted for this principal.
    pub audience: String,
    /// How the principal was authenticated (e.g. `workload-identity`, `mtls`, `dev`).
    pub auth_method: String,
    /// The credential / key id that authenticated it (e.g. the identity signing key).
    #[serde(default)]
    pub credential_id: String,
    /// Correlation id for audit.
    #[serde(default)]
    pub correlation_id: String,
}

/// What kind of principal an identity represents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityKind {
    Human,
    #[default]
    Workload,
    Session,
    Task,
}

/// The signed assertion of *who or what* is acting, before any authority (token)
/// is granted. Superset-compatible with the MAI claim subject fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub identity_id: String,
    #[serde(default)]
    pub kind: IdentityKind,
    pub tenant_id: String,
    #[serde(default)]
    pub subject_id: String,
    pub subject_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_identity: Option<String>,
    #[serde(default)]
    pub spiffe_id: String,
    #[serde(default)]
    pub pki_cert_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub signature: Signature,
}
