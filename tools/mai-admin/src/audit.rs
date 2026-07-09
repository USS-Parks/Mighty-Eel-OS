//! Audit entry + chain verifier vendored from `mai-api/src/audit.rs`.
//!
//! Why not depend on mai-api? See the matching note in
//! `tools/mai-admin/src/profile.rs`. Both modules go away once
//! the endpoint-and-cli session lands and mai-api is buildable in the
//! shared workspace again.
//!
//! Compatibility contract: the on-disk JSON line format must match the
//! one `mai-api`'s `WalAuditWriter` produces. The `verify_chain`
//! formula here MUST be byte-identical to the one in mai-api —
//! changing either side without updating the other invalidates every
//! existing audit chain in the field.

use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

/// SHA3-256(zeros) sentinel matching `mai-api/src/audit.rs::GENESIS_HASH`.
/// First WAL entry chains from this hash.
pub const GENESIS_HASH: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

/// A single audit log entry with hash chain linkage. Mirrors
/// `mai_api::audit::AuditEntry` minus optional metadata fields we
/// don't need here. We use `#[serde(default)]` on the optional fields
/// so the on-disk JSON from the live API still parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub entry_id: String,
    pub timestamp: u64,
    pub previous_hash: String,
    pub entry_hash: String,
    pub profile_id: String,
    #[serde(default)]
    pub profile_role: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub request_type: Option<serde_json::Value>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub pqc_signature: Option<String>,
}

fn compute_entry_hash(
    previous_hash: &str,
    timestamp: u64,
    profile_id: &str,
    method: &str,
    path: &str,
    status_code: u16,
) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(previous_hash.as_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(profile_id.as_bytes());
    hasher.update(method.as_bytes());
    hasher.update(path.as_bytes());
    hasher.update(status_code.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Verify the integrity of an audit chain. Returns `Ok(count)` or
/// `Err((index, detail))` on the first broken link. Byte-identical to
/// `mai_api::audit::verify_chain`.
pub fn verify_chain(entries: &[AuditEntry]) -> Result<usize, (usize, String)> {
    if entries.is_empty() {
        return Ok(0);
    }
    if entries[0].previous_hash != GENESIS_HASH {
        return Err((
            0,
            "First entry does not chain from genesis hash".to_string(),
        ));
    }
    for (i, entry) in entries.iter().enumerate() {
        let expected = compute_entry_hash(
            &entry.previous_hash,
            entry.timestamp,
            &entry.profile_id,
            &entry.method,
            &entry.path,
            entry.status_code,
        );
        if entry.entry_hash != expected {
            return Err((
                i,
                format!(
                    "Hash mismatch at entry {i}: expected {expected}, got {}",
                    entry.entry_hash
                ),
            ));
        }
        if i + 1 < entries.len() && entries[i + 1].previous_hash != entry.entry_hash {
            return Err((
                i + 1,
                format!(
                    "Chain broken at entry {}: previous_hash does not match prior entry",
                    i + 1
                ),
            ));
        }
    }
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(prev: &str, ts: u64, profile: &str, path: &str, status: u16) -> AuditEntry {
        let entry_hash = compute_entry_hash(prev, ts, profile, "GET", path, status);
        AuditEntry {
            entry_id: format!("entry-{ts}"),
            timestamp: ts,
            previous_hash: prev.to_string(),
            entry_hash,
            profile_id: profile.to_string(),
            profile_role: "operator".to_string(),
            method: "GET".to_string(),
            path: path.to_string(),
            status_code: status,
            duration_ms: 0,
            model_name: None,
            request_type: None,
            context: None,
            pqc_signature: None,
        }
    }

    #[test]
    fn empty_chain_is_ok() {
        assert_eq!(verify_chain(&[]).unwrap(), 0);
    }

    #[test]
    fn single_entry_chains_from_genesis() {
        let e = sample_entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
        assert_eq!(verify_chain(&[e]).unwrap(), 1);
    }

    #[test]
    fn chain_of_two_links_correctly() {
        let e0 = sample_entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
        let e1 = sample_entry(&e0.entry_hash, 1001, "bob", "/v1/chat", 200);
        assert_eq!(verify_chain(&[e0, e1]).unwrap(), 2);
    }

    #[test]
    fn first_entry_must_chain_from_genesis() {
        let mut e = sample_entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
        e.previous_hash = "deadbeef".to_string();
        e.entry_hash = compute_entry_hash("deadbeef", 1000, "alice", "GET", "/v1/health", 200);
        let err = verify_chain(&[e]).unwrap_err();
        assert_eq!(err.0, 0);
    }

    #[test]
    fn tampered_entry_hash_is_detected() {
        let mut e = sample_entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
        e.entry_hash = "00".repeat(32);
        let err = verify_chain(&[e]).unwrap_err();
        assert!(err.1.contains("Hash mismatch"));
    }

    #[test]
    fn broken_linkage_is_detected() {
        let e0 = sample_entry(GENESIS_HASH, 1000, "alice", "/v1/health", 200);
        let mut e1 = sample_entry(&e0.entry_hash, 1001, "bob", "/v1/chat", 200);
        e1.previous_hash = "deadbeef".to_string();
        e1.entry_hash = compute_entry_hash("deadbeef", 1001, "bob", "GET", "/v1/chat", 200);
        let err = verify_chain(&[e0, e1]).unwrap_err();
        assert_eq!(err.0, 1);
        assert!(err.1.contains("Chain broken"));
    }
}
