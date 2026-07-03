//! Envelope contract — see `contracts/envelope.md`. Three wraps: seal (can't read
//! it), label (act on it without reading it), thread (prove where it came from).

use serde::{Deserialize, Serialize};

use crate::common::{Classification, ComplianceScope, Route, Signature};

/// Wrap 1: the encrypted payload. The data key is wrapped by OpenBao transit; the
/// ciphertext is AEAD. Opening it needs a token and the trust boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seal {
    pub aead_alg: String,
    pub data_key_wrapped: String,
    pub nonce: String,
    pub ciphertext: String,
    pub aad_hash: String,
}

/// Wrap 2: the handling label. Machine-readable **without** unsealing — this is
/// what AOG reads for DSPM-informed routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub classification: Classification,
    #[serde(default)]
    pub compliance_scopes: Vec<ComplianceScope>,
    pub origin: String,
    #[serde(default)]
    pub permitted_ops: Vec<String>,
    #[serde(default)]
    pub permitted_destinations: Vec<Route>,
    #[serde(default)]
    pub detected_entities: Vec<String>,
}

/// Wrap 3: the provenance thread. Authorizing token, chain link, signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub created_at: String,
    pub authorizing_token_id: String,
    pub previous_hash: String,
    #[serde(default)]
    pub signatures: Vec<Signature>,
}

/// A sealed, labelled, threaded envelope — the only way regulated data moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub envelope_id: String,
    pub seal: Seal,
    pub label: Label,
    pub thread: Thread,
}
