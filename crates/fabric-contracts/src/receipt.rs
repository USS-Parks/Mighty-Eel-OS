//! Receipt contract — see `contracts/receipt.md`. Extends the MAI `AuditEntry`
//! with the token/envelope/spend/model-digest/workflow strands WSF and AOG need.
//! Only hashes, ids, and metadata — never a regulated payload.

use serde::{Deserialize, Serialize};

use crate::common::RoutingDecision;

/// BF-5 correlation fields, joined across cloud credential events and local
/// decisions. `token_id` accepts the legacy `claim_id` name on read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correlation {
    #[serde(default)]
    pub credential_event_id: String,
    #[serde(default)]
    pub lamprey_decision_id: String,
    #[serde(default)]
    pub mai_request_id: String,
    #[serde(default)]
    pub subject_hash: String,
    #[serde(default, alias = "claim_id")]
    pub token_id: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub bundle_version: String,
    #[serde(default)]
    pub service_identity: String,
    #[serde(default)]
    pub offline_mode: bool,
}

/// An ML-DSA-87 signature applied every N receipts, verifiable off-host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodicSignature {
    pub alg: String,
    pub key_id: String,
    pub value: String,
    pub covers_through: String,
}

/// A single hash-chain node recording one governed action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_id: String,
    pub request_id: String,
    pub request_hash: String,
    pub previous_hash: String,
    pub routing_decision: RoutingDecision,
    #[serde(default)]
    pub modules_applied: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub correlation: Correlation,
    pub token_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_weights_digest: Option<String>,
    #[serde(default)]
    pub spend_cents: u64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    pub recorded_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub periodic_signature: Option<PeriodicSignature>,
}
