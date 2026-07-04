//! Tool-call receipts (T1) — a metadata-only, verifiable provenance chain.
//!
//! Every tool call brokered by the proxy appends a [`ToolReceipt`] to a BLAKE3
//! hash chain (`fabric-proof`, the same primitive the AOG meter and WSF ledger
//! use), so the tool-governance audit trail verifies off-host and tampering is
//! detectable. Receipts carry metadata only — never the tool arguments or output.

use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use serde::Serialize;

/// A metadata-only receipt for one brokered tool call.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolReceipt {
    pub call_id: String,
    pub tool_id: String,
    pub session_id: String,
    /// The authorizing subject/token id (never the credential itself).
    pub profile_id: String,
    /// Whether the tool mutates state (a side-effecting / "write" tool).
    pub has_side_effects: bool,
    pub success: bool,
    pub duration_ms: u64,
    pub chain_step: u32,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The ephemeral credential lease minted for this call (T2) — the id only,
    /// never the secret; `None` when no minter is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cred_lease: Option<String>,
    /// The minted credential's TTL in ms (the call's lifetime).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cred_ttl_ms: Option<u64>,
    /// The call was side-effecting and routed through the approval inbox (T3).
    #[serde(default, skip_serializing_if = "is_false")]
    pub approval_required: bool,
    /// Who approved a gated call (the actor); `None` for un-gated or blocked calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

/// `skip_serializing_if` predicate — omit a `false` flag so an un-gated receipt is
/// unchanged.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Append-only tool-call receipt ledger: a BLAKE3 chain over [`ToolReceipt`]s.
#[derive(Debug, Default)]
pub struct ToolReceiptChain {
    links: Vec<ChainLink>,
    receipts: Vec<ToolReceipt>,
    last_hash: [u8; 32],
}

impl ToolReceiptChain {
    #[must_use]
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            receipts: Vec::new(),
            last_hash: GENESIS_HASH,
        }
    }

    /// Append a receipt; returns the new chain head (hex).
    pub fn append(&mut self, receipt: ToolReceipt) -> String {
        let value = serde_json::to_value(&receipt).expect("tool receipt serializes");
        let entry_hash = canonical_hash(&value).expect("canonical hash of tool receipt");
        self.links.push(ChainLink {
            previous_hash: self.last_hash,
            entry_hash,
        });
        self.last_hash = chain_link(&self.last_hash, &entry_hash);
        self.receipts.push(receipt);
        hex::encode(self.last_hash)
    }

    /// Verify the chain is unbroken from genesis.
    #[must_use]
    pub fn verify(&self) -> bool {
        verify_chain(&self.links).is_ok()
    }

    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.last_hash)
    }

    #[must_use]
    pub fn receipts(&self) -> &[ToolReceipt] {
        &self.receipts
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
}
