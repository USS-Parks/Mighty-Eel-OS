//! Trust-token contract — see `contracts/trust-token.md`. The WSF primitive:
//! the MAI `SignedClaim` plus a budget strand and attenuation caveats. A wire
//! superset of the MAI claim (an old claim is a root token with no budget).

use serde::{Deserialize, Serialize};

use crate::common::{Classification, ComplianceScope, RevocationStatus, Route, Signature};

/// Spend ceilings carried in the token itself. Absent `budget` on a token means
/// budget enforcement is off (legacy-claim compatibility); the bridge always
/// populates it for new tokens. Enforcement lives in `fabric-token`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub token_cap: u64,
    #[serde(default)]
    pub tokens_spent: u64,
    #[serde(default)]
    pub usd_cap_cents: u64,
    #[serde(default)]
    pub usd_spent_cents: u64,
    #[serde(default)]
    pub tool_call_cap: u32,
    #[serde(default)]
    pub tool_calls_spent: u32,
}

/// The axis a caveat narrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaveatType {
    RouteCeiling,
    ModelAllowlist,
    ResourcePrefix,
    ToolAllowlist,
    ExpiryBefore,
    ClassificationCeiling,
}

/// A single narrowing predicate applied when minting a child token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caveat {
    #[serde(rename = "type")]
    pub caveat_type: CaveatType,
    pub value: String,
}

/// Attenuation lineage: the parent this token was minted from, the caveats that
/// narrowed it, and the lineage depth. A root token has `parent_id: None`, no
/// caveats, and `depth: 0`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attenuation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub caveats: Vec<Caveat>,
    /// Lineage depth: 0 for a root token, `parent.depth + 1` for each attenuated
    /// child. `fabric-token` bounds it (see `MAX_ATTENUATION_DEPTH`) so a chain
    /// cannot grow without limit. Omitted from the canonical payload when 0, so a
    /// root token's signed bytes — and every pre-existing signature — are
    /// unchanged.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub depth: u32,
}

/// serde `skip_serializing_if` predicate: omit a `u32` field when it is 0.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// The trust token. Field order and names mirror the MAI claim so existing
/// signatures over the canonical payload continue to verify (`fabric-proof`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustToken {
    #[serde(alias = "claim_id")]
    pub token_id: String,
    pub issued_at: String,
    pub expires_at: String,
    pub issuer: String,
    pub trust_bundle_version: String,

    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    pub subject_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,

    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub compliance_scopes: Vec<ComplianceScope>,
    #[serde(default)]
    pub allowed_routes: Vec<Route>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    pub max_data_classification: Classification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_type: Option<String>,
    #[serde(default)]
    pub offline_mode: bool,
    #[serde(default = "unknown_revocation")]
    pub revocation_status: RevocationStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    #[serde(default)]
    pub attenuation: Attenuation,

    pub signature: Signature,
}

fn unknown_revocation() -> RevocationStatus {
    RevocationStatus::Unknown
}
