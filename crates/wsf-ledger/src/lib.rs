//! `wsf-ledger` — the WSF append-only receipt ledger + signed evidence packs.
//!
//! Every trust-plane service (the bridge, the seal service, later the gateway)
//! emits **metadata-only** receipts. The ledger ingests them from any source
//! into one BLAKE3 hash chain (`fabric-proof`), lets you query by correlation
//! field, and **exports an ML-DSA-signed evidence pack** that verifies off-host
//! with a public key alone — the "one evidence lake" a regulator can check
//! without touching the running system.
//!
//! Receipts are ingested as canonical JSON (`serde_json::Value`) so the ledger
//! is agnostic to each service's receipt shape (a `SealReceipt`, an
//! `AuditCorrelation`, …) while still chaining and signing them uniformly.
//!
//! Evidence-pack *formatting* (the certified report layout from
//! `mai-compliance/src/reports/*`) is a later C9/D4 concern; W4 owns the
//! append-only chain + the off-host-verifiable signature, which is what the gate
//! requires.

use std::sync::Arc;

use fabric_contracts::Signature;
use fabric_crypto::{Signer, Verifier};
use fabric_proof::{ChainLink, GENESIS_HASH, canonical_hash, chain_link, verify_chain};
use serde::{Deserialize, Serialize};

/// Failures from ledger operations.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// A receipt could not be canonically hashed.
    #[error("canonical hash failed: {0}")]
    Hash(String),
    /// The stored chain does not verify.
    #[error("chain broken: {0}")]
    ChainBroken(String),
    /// Evidence-pack signing failed.
    #[error("signing failed: {0}")]
    Sign(String),
}

/// One ingested receipt with its position in the chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Zero-based sequence number.
    pub seq: u64,
    /// Emitting service (e.g. `wsf-seal`, `wsf-bridge`).
    pub source: String,
    /// The canonical receipt.
    pub receipt: serde_json::Value,
    /// The chain hash this entry extends, hex.
    pub previous_hash: String,
    /// BLAKE3 hash of this entry's canonical receipt, hex.
    pub entry_hash: String,
}

/// A signed, exportable evidence pack over a ledger snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    /// The receipts, in chain order.
    pub entries: Vec<LedgerEntry>,
    /// The chain head at export, hex.
    pub head_hash: String,
    /// Entry count.
    pub count: usize,
    /// Export time (RFC3339).
    pub generated_at: String,
    /// ML-DSA signature over the pack (with this field blanked).
    pub signature: Signature,
}

/// An append-only receipt ledger.
pub struct Ledger {
    entries: Vec<LedgerEntry>,
    last_hash: [u8; 32],
    signer: Arc<dyn Signer>,
}

impl Ledger {
    /// A fresh ledger anchored at genesis, signing packs with `signer`.
    #[must_use]
    pub fn new(signer: Arc<dyn Signer>) -> Self {
        Self {
            entries: Vec::new(),
            last_hash: GENESIS_HASH,
            signer,
        }
    }

    /// Ingest a canonical `receipt` from `source`. Returns the new chain head (hex).
    ///
    /// # Errors
    /// [`LedgerError::Hash`] if the receipt cannot be canonically hashed.
    pub fn ingest(
        &mut self,
        source: impl Into<String>,
        receipt: serde_json::Value,
    ) -> Result<String, LedgerError> {
        let entry_hash = canonical_hash(&receipt).map_err(|e| LedgerError::Hash(e.to_string()))?;
        let previous_hash = self.last_hash;
        self.last_hash = chain_link(&previous_hash, &entry_hash);
        let seq = self.entries.len() as u64;
        self.entries.push(LedgerEntry {
            seq,
            source: source.into(),
            receipt,
            previous_hash: hex::encode(previous_hash),
            entry_hash: hex::encode(entry_hash),
        });
        Ok(hex::encode(self.last_hash))
    }

    /// Verify the whole chain from genesis. Returns the head hash on success.
    ///
    /// # Errors
    /// [`LedgerError::ChainBroken`] if any link is malformed or the chain breaks.
    pub fn verify(&self) -> Result<String, LedgerError> {
        let links = self
            .entries
            .iter()
            .map(decode_link)
            .collect::<Result<Vec<_>, _>>()?;
        let head = verify_chain(&links).map_err(|e| LedgerError::ChainBroken(e.to_string()))?;
        Ok(hex::encode(head))
    }

    /// Entries whose receipt has a top-level string `field == value` — the
    /// correlation query (by `token_id`, `envelope_id`, `tenant_id`, …).
    ///
    /// This is an unfiltered library primitive; the authenticated API surface must
    /// use [`query_tenant`](Ledger::query_tenant) / [`query_global`](Ledger::query_global)
    /// so a caller can never read another tenant's receipts (AF-007).
    #[must_use]
    pub fn query(&self, field: &str, value: &str) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|e| e.receipt.get(field).and_then(serde_json::Value::as_str) == Some(value))
            .collect()
    }

    /// Tenant-scoped receipt query (AF-007): entries whose receipt carries a
    /// top-level `tenant_id == tenant`, optionally further filtered by `token_id`,
    /// capped at `limit`. A receipt without a `tenant_id` is **never** returned to
    /// a tenant query — unattributable metadata is not disclosed cross-tenant, and
    /// there is no existence oracle (a non-matching id yields no rows, not an
    /// error).
    #[must_use]
    pub fn query_tenant(
        &self,
        tenant: &str,
        token_id: Option<&str>,
        limit: usize,
    ) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.receipt
                    .get("tenant_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(tenant)
            })
            .filter(|e| {
                token_id.is_none_or(|t| {
                    e.receipt
                        .get("token_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(t)
                })
            })
            .take(limit)
            .collect()
    }

    /// Global-auditor query: every tenant's entries, optionally filtered by
    /// `token_id`, capped at `limit`. Only a separately-audited global-auditor
    /// principal may reach this (enforced at the API layer).
    #[must_use]
    pub fn query_global(&self, token_id: Option<&str>, limit: usize) -> Vec<&LedgerEntry> {
        self.entries
            .iter()
            .filter(|e| {
                token_id.is_none_or(|t| {
                    e.receipt
                        .get("token_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(t)
                })
            })
            .take(limit)
            .collect()
    }

    /// Number of ingested receipts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current chain head, hex.
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.last_hash)
    }

    /// The pack-signing public key (for off-host [`verify_pack`]).
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.signer.public_key()
    }

    /// All entries, in order.
    #[must_use]
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    /// Export a signed evidence pack over the current ledger state.
    ///
    /// # Errors
    /// [`LedgerError::Hash`] or [`LedgerError::Sign`] on hashing / signing failure.
    pub fn export_pack(
        &self,
        generated_at: impl Into<String>,
    ) -> Result<EvidencePack, LedgerError> {
        let mut pack = EvidencePack {
            entries: self.entries.clone(),
            head_hash: self.head_hex(),
            count: self.entries.len(),
            generated_at: generated_at.into(),
            signature: Signature {
                alg: self.signer.algorithm().to_string(),
                key_id: self.signer.key_id().to_string(),
                value: String::new(),
            },
        };
        let hash = pack_hash(&pack).map_err(LedgerError::Hash)?;
        let sig = self
            .signer
            .sign(&hash)
            .map_err(|e| LedgerError::Sign(e.to_string()))?;
        pack.signature.value = hex::encode(sig);
        Ok(pack)
    }
}

fn decode_link(entry: &LedgerEntry) -> Result<ChainLink, LedgerError> {
    Ok(ChainLink {
        previous_hash: decode32(&entry.previous_hash)?,
        entry_hash: decode32(&entry.entry_hash)?,
    })
}

fn decode32(s: &str) -> Result<[u8; 32], LedgerError> {
    hex::decode(s)
        .map_err(|e| LedgerError::ChainBroken(e.to_string()))?
        .try_into()
        .map_err(|_| LedgerError::ChainBroken("hash is not 32 bytes".to_string()))
}

/// Canonical hash of a pack with its signature value blanked.
fn pack_hash(pack: &EvidencePack) -> Result<[u8; 32], String> {
    let mut v = serde_json::to_value(pack).map_err(|e| e.to_string())?;
    if let Some(value) = v.get_mut("signature").and_then(|s| s.get_mut("value")) {
        *value = serde_json::Value::String(String::new());
    }
    canonical_hash(&v).map_err(|e| e.to_string())
}

/// Verify an evidence pack's signature off-host (public key only, no ledger).
#[must_use]
pub fn verify_pack(pack: &EvidencePack, verifier: &dyn Verifier, public_key: &[u8]) -> bool {
    let Ok(hash) = pack_hash(pack) else {
        return false;
    };
    let Ok(sig) = hex::decode(&pack.signature.value) else {
        return false;
    };
    verifier.verify(&hash, &sig, public_key).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
    use serde_json::json;

    fn ledger() -> Ledger {
        Ledger::new(Arc::new(RustCryptoMlDsa87::generate("ledger-key").unwrap()))
    }

    #[test]
    fn multi_source_receipts_chain_correctly() {
        let mut l = ledger();
        l.ingest("wsf-bridge", json!({"token_id":"tok_1","op":"issue"}))
            .unwrap();
        l.ingest(
            "wsf-seal",
            json!({"token_id":"tok_1","envelope_id":"env_1","op":"seal"}),
        )
        .unwrap();
        l.ingest(
            "wsf-seal",
            json!({"token_id":"tok_1","envelope_id":"env_1","op":"unseal"}),
        )
        .unwrap();
        assert_eq!(l.len(), 3);
        l.verify().expect("chain verifies");
        // Correlation query joins across sources.
        assert_eq!(l.query("token_id", "tok_1").len(), 3);
        assert_eq!(l.query("envelope_id", "env_1").len(), 2);
        assert_eq!(l.query("token_id", "nope").len(), 0);
    }

    #[test]
    fn exported_pack_verifies_off_host() {
        let mut l = ledger();
        l.ingest("wsf-seal", json!({"envelope_id":"e1","decision":"allow"}))
            .unwrap();
        l.ingest("wsf-seal", json!({"envelope_id":"e1","decision":"deny"}))
            .unwrap();
        let pack = l.export_pack("2026-07-03T00:00:00Z").unwrap();
        assert!(verify_pack(&pack, &MlDsa87Verifier, l.public_key()));
        // Wrong key → fails.
        let other = RustCryptoMlDsa87::generate("other").unwrap();
        assert!(!verify_pack(&pack, &MlDsa87Verifier, other.public_key()));
    }

    #[test]
    fn tampered_pack_fails_verification() {
        let mut l = ledger();
        l.ingest("wsf-seal", json!({"envelope_id":"e1","decision":"allow"}))
            .unwrap();
        let mut pack = l.export_pack("2026-07-03T00:00:00Z").unwrap();
        // Flip a receipt after signing.
        pack.entries[0].receipt = json!({"envelope_id":"e1","decision":"deny"});
        assert!(!verify_pack(&pack, &MlDsa87Verifier, l.public_key()));
    }

    #[test]
    fn tampered_chain_fails_verify() {
        let mut l = ledger();
        l.ingest("s", json!({"a":"1"})).unwrap();
        l.ingest("s", json!({"a":"2"})).unwrap();
        // Break the second link's back-pointer.
        l.entries[1].previous_hash = hex::encode([9u8; 32]);
        assert!(l.verify().is_err());
    }

    #[test]
    fn tenant_scoped_query_isolates_tenants() {
        let mut l = ledger();
        l.ingest("wsf-bridge", json!({"tenant_id":"a","token_id":"tok_a"}))
            .unwrap();
        l.ingest("wsf-bridge", json!({"tenant_id":"b","token_id":"tok_b"}))
            .unwrap();
        l.ingest("wsf-seal", json!({"token_id":"tok_x"})).unwrap(); // no tenant_id

        // Tenant a sees only its own row.
        let a = l.query_tenant("a", None, 100);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].receipt["token_id"], "tok_a");
        // No cross-tenant leak and no existence oracle: querying tenant b's token
        // as tenant a returns nothing (not an error, not a "hidden" hint).
        assert_eq!(l.query_tenant("a", Some("tok_b"), 100).len(), 0);
        // A receipt with no tenant_id is never returned to a tenant query.
        assert_eq!(l.query_tenant("a", Some("tok_x"), 100).len(), 0);
        // The global auditor sees everything; the limit caps results.
        assert_eq!(l.query_global(None, 100).len(), 3);
        assert_eq!(l.query_global(None, 2).len(), 2);
    }
}
