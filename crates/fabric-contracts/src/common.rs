//! Shared value types used across all four contracts.

use serde::{Deserialize, Serialize};

/// A detached signature over a canonical payload. Verification lives in
/// `fabric-proof`; here it is opaque bytes plus the key/alg that produced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub alg: String,
    pub key_id: String,
    pub value: String,
}

/// Routing ceiling / permitted destination, shared by tokens and envelope labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    LocalOnly,
    LocalPreferred,
    CloudAllowed,
}

/// Data-classification ladder, ordered least → most sensitive. The derived `Ord`
/// gives declaration order, which `fabric-token` uses for the ceiling check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    Public,
    Internal,
    Restricted,
    Controlled,
    Secret,
}

/// Compliance regimes a tenant may license (`TRUST-MANIFOLD.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceScope {
    Hipaa,
    ItarEar,
    Ocap,
}

/// Revocation state of a token/claim at evaluation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevocationStatus {
    Valid,
    Revoked,
    Stale,
    Unknown,
}

/// Receipt routing outcome. Wire form is PascalCase to match the MAI `AuditEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingDecision {
    Allow,
    LocalOnly,
    Quarantine,
    Deny,
}
