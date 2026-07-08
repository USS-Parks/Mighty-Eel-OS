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

/// Tenant/owner/audience binding on an envelope (plan E1). Readable without
/// unsealing (so unseal can authorize *before* decrypt) and folded into the
/// AEAD's AAD (so no field can be swapped without breaking the seal). An empty
/// `tenant_id` marks a legacy (v1) unbound envelope — online unseal of those is
/// denied unless a bounded migration is enabled (E5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeBinding {
    /// Owning tenant. A presenting token from another tenant cannot unseal.
    #[serde(default)]
    pub tenant_id: String,
    /// Pseudonymized owner subject the payload was sealed for.
    #[serde(default)]
    pub owner_subject_hash: String,
    /// Plane the envelope may be opened on (e.g. `wsf`).
    #[serde(default)]
    pub audience: String,
    /// Envelope contract version (`2` = tenant-bound v2; `0`/absent = legacy v1).
    #[serde(default)]
    pub envelope_version: u32,
}

/// A sealed, labelled, threaded envelope — the only way regulated data moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub envelope_id: String,
    pub seal: Seal,
    pub label: Label,
    pub thread: Thread,
    /// Tenant/owner/audience binding (plan E1). `default` for legacy envelopes.
    #[serde(default)]
    pub binding: EnvelopeBinding,
}
