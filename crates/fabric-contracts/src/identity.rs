//! Identity contract — see `contracts/identity.md`.

use serde::{Deserialize, Serialize};

use crate::common::Signature;

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
