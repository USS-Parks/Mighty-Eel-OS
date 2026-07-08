//! Vault-level audit trail writer.
//!
//! Implements `AuditStore` with hash-chained, PQC-signed entries.
//! Uses in-memory storage with JSON persistence (production: SQLite WAL).
//!
//! # Hash Chain
//!
//! Each entry's `entry_hash` is BLAKE3 over the canonical serialization of the
//! entry with its two output fields (`entry_hash`, `pqc_signature`) blanked, so
//! every security-relevant field is bound into the chain — not just a subset.
//!
//! The chain starts with a genesis hash of all zeros.
//!
//! # PQC Signatures
//!
//! Every Nth entry (configurable, default 100) is signed with ML-DSA-87
//! to create cryptographic checkpoints. These checkpoints allow verification
//! of chain integrity without re-verifying every individual entry.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{
    AuditStore, ComplianceReport, PqcProvider, VaultAuditAction, VaultAuditEntry, VaultAuditStatus,
    VaultError,
};

use crate::config::AuditConfig;
use crate::pqc::PqcEngine;

/// Genesis hash (all zeros) for the first entry in the chain.
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Hex-encode bytes (lowercase). Local helper — the `blake3` crate already
/// provides hex output for hashes, but raw ML-DSA signatures need plain hex.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Decode a lowercase-hex string. Returns `None` on malformed input.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Vault audit trail writer.
///
/// Maintains an append-only, hash-chained sequence of audit entries.
/// Entries are stored in memory and persisted to disk on append.
///
/// When constructed via [`AuditWriter::with_pqc`], the writer signs the
/// chain head with ML-DSA-87 once every [`AuditConfig::sign_interval`]
/// appended entries. Each checkpoint signature is stored in the
/// `pqc_signature` field of the entry that triggered it.
pub struct AuditWriter {
    config: AuditConfig,
    /// Ordered list of audit entries (append-only).
    entries: RwLock<Vec<VaultAuditEntry>>,
    /// Hash of the most recent entry.
    last_hash: RwLock<String>,
    /// Optional PQC engine for periodic chain-head signatures.
    pqc: Option<Arc<PqcEngine>>,
}

impl AuditWriter {
    /// Create a new audit writer with no checkpoint signing.
    pub fn new(config: AuditConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(Vec::new()),
            last_hash: RwLock::new(GENESIS_HASH.to_string()),
            pqc: None,
        }
    }

    /// Create an audit writer that signs the chain head every
    /// [`AuditConfig::sign_interval`] entries using `pqc`.
    pub fn with_pqc(config: AuditConfig, pqc: Arc<PqcEngine>) -> Self {
        Self {
            config,
            entries: RwLock::new(Vec::new()),
            last_hash: RwLock::new(GENESIS_HASH.to_string()),
            pqc: Some(pqc),
        }
    }

    /// Initialize: load existing entries from disk if the database exists.
    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            db_path = %self.config.db_path.display(),
            "Initializing audit trail"
        );

        if self.config.db_path.exists() {
            match std::fs::read_to_string(&self.config.db_path) {
                Ok(content) => match serde_json::from_str::<Vec<VaultAuditEntry>>(&content) {
                    Ok(loaded) => {
                        let mut entries = self.entries.write().await;
                        let mut last = self.last_hash.write().await;

                        if let Some(final_entry) = loaded.last() {
                            (*last).clone_from(&final_entry.entry_hash);
                        }
                        *entries = loaded;
                        info!(
                            count = entries.len(),
                            last_hash = %*last,
                            "Loaded audit trail from disk"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "Could not parse audit database, starting fresh");
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Could not read audit database, starting fresh");
                }
            }
        }

        Ok(())
    }

    /// Compute the hash for an audit entry.
    ///
    /// BLAKE3 over the canonical serialization of **every security-relevant
    /// field** — the whole entry with its two output fields (`entry_hash` and
    /// `pqc_signature`) blanked. This binds `entry_id`, `model_id`, token counts,
    /// `latency_ms`, `adapter_id`, `error_code`, and `ip_source` into the chain,
    /// not just the original five (audit H6: editing any persisted field now
    /// breaks `verify`). Producer and verifier share this function, so the digest
    /// is self-consistent across the chain.
    fn compute_entry_hash(entry: &VaultAuditEntry) -> String {
        let mut canonical = entry.clone();
        canonical.entry_hash = String::new();
        canonical.pqc_signature = None;
        let bytes =
            serde_json::to_vec(&canonical).expect("VaultAuditEntry serialization is infallible");
        blake3::hash(&bytes).to_hex().to_string()
    }

    /// Persist entries to disk.
    async fn persist(&self) -> Result<(), VaultError> {
        let entries = self.entries.read().await;
        let json = serde_json::to_string(&*entries)
            .map_err(|e| VaultError::AuditStoreError(e.to_string()))?;

        if let Some(parent) = self.config.db_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| VaultError::AuditStoreError(e.to_string()))?;
        }

        std::fs::write(&self.config.db_path, json)
            .map_err(|e| VaultError::AuditStoreError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl AuditStore for AuditWriter {
    async fn append(&self, entry: &VaultAuditEntry) -> Result<(), VaultError> {
        let mut entries = self.entries.write().await;
        let mut last = self.last_hash.write().await;

        // Verify the entry chains correctly
        if entry.previous_hash != *last {
            return Err(VaultError::AuditChainBroken {
                index: entries.len() as u64,
                detail: format!(
                    "Entry previous_hash {} does not match chain tip {}",
                    entry.previous_hash, *last
                ),
            });
        }

        // Verify the entry hash is correct
        let expected_hash = Self::compute_entry_hash(entry);
        if entry.entry_hash != expected_hash {
            return Err(VaultError::AuditChainBroken {
                index: entries.len() as u64,
                detail: format!(
                    "Entry hash mismatch: got {}, expected {expected_hash}",
                    entry.entry_hash
                ),
            });
        }

        debug!(
            entry_id = %entry.entry_id,
            action = ?entry.action,
            "Appending audit entry"
        );

        // Sign the chain head every `sign_interval` entries when a PQC engine
        // is wired. The resulting ML-DSA-87 signature is stored as hex on the
        // entry that triggered the checkpoint.
        let new_count = (entries.len() as u64).saturating_add(1);
        let mut stored = entry.clone();
        if let Some(pqc) = self.pqc.as_ref() {
            let interval = self.config.sign_interval.max(1);
            if new_count.is_multiple_of(interval) {
                match pqc.sign_package(entry.entry_hash.as_bytes()).await {
                    Ok(sig) => {
                        stored.pqc_signature = Some(hex_encode(&sig));
                        info!(
                            entry_index = new_count,
                            interval, "Audit chain checkpoint signed (ML-DSA-87)"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to sign audit checkpoint; entry stored unsigned");
                    }
                }
            }
        }

        (*last).clone_from(&stored.entry_hash);
        entries.push(stored);
        drop(entries);
        drop(last);

        self.persist().await?;
        Ok(())
    }

    async fn read_recent(&self, count: usize) -> Result<Vec<VaultAuditEntry>, VaultError> {
        let entries = self.entries.read().await;
        let start = entries.len().saturating_sub(count);
        Ok(entries[start..].to_vec())
    }

    async fn read_by_profile(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<VaultAuditEntry>, VaultError> {
        let entries = self.entries.read().await;
        let result: Vec<VaultAuditEntry> = entries
            .iter()
            .rev()
            .filter(|e| e.profile_id == profile_id)
            .take(limit)
            .cloned()
            .collect();
        Ok(result)
    }

    async fn read_by_time_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<VaultAuditEntry>, VaultError> {
        let entries = self.entries.read().await;
        let result: Vec<VaultAuditEntry> = entries
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .cloned()
            .collect();
        Ok(result)
    }

    async fn verify_chain(&self) -> Result<u64, VaultError> {
        let entries = self.entries.read().await;
        let mut expected_prev = GENESIS_HASH.to_string();

        for (i, entry) in entries.iter().enumerate() {
            // Check previous_hash linkage
            if entry.previous_hash != expected_prev {
                return Err(VaultError::AuditChainBroken {
                    index: i as u64,
                    detail: format!(
                        "previous_hash mismatch at index {i}: expected {expected_prev}, got {}",
                        entry.previous_hash
                    ),
                });
            }

            // Recompute and verify entry hash
            let computed = Self::compute_entry_hash(entry);
            if entry.entry_hash != computed {
                return Err(VaultError::AuditChainBroken {
                    index: i as u64,
                    detail: format!(
                        "entry_hash mismatch at index {i}: expected {computed}, got {}",
                        entry.entry_hash
                    ),
                });
            }

            // Verify ML-DSA-87 checkpoint signature when present.
            if let (Some(sig_hex), Some(pqc)) = (&entry.pqc_signature, self.pqc.as_ref()) {
                let sig = hex_decode(sig_hex).ok_or_else(|| VaultError::AuditChainBroken {
                    index: i as u64,
                    detail: format!("malformed pqc_signature hex at index {i}"),
                })?;
                let ok = pqc
                    .verify_package(entry.entry_hash.as_bytes(), &sig)
                    .await?;
                if !ok {
                    return Err(VaultError::AuditChainBroken {
                        index: i as u64,
                        detail: format!("ML-DSA-87 checkpoint signature invalid at index {i}"),
                    });
                }
            }

            expected_prev.clone_from(&entry.entry_hash);
        }

        Ok(entries.len() as u64)
    }

    async fn export_compliance(
        &self,
        start: u64,
        end: u64,
    ) -> Result<ComplianceReport, VaultError> {
        let entries = self.entries.read().await;

        let mut action_summary: HashMap<String, u64> = HashMap::new();
        let mut profile_summary: HashMap<String, u64> = HashMap::new();
        let mut count = 0u64;

        for entry in entries.iter() {
            if entry.timestamp >= start && entry.timestamp <= end {
                count += 1;
                *action_summary
                    .entry(format!("{:?}", entry.action))
                    .or_insert(0) += 1;
                *profile_summary.entry(entry.profile_id.clone()).or_insert(0) += 1;
            }
        }

        // Verify chain integrity
        drop(entries); // release lock before verify_chain
        let chain_verified = match self.verify_chain().await {
            Ok(n) => (n, true),
            Err(_) => (0, false),
        };

        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
        let generated_at = chrono::Utc::now().timestamp() as u64;
        Ok(ComplianceReport {
            generated_at,
            range_start: start,
            range_end: end,
            total_entries: count,
            action_summary,
            profile_summary,
            chain_verified_to: chain_verified.0,
            chain_intact: chain_verified.1,
            report_signature: None, // PQC signature added when PqcEngine is wired
        })
    }

    async fn entry_count(&self) -> Result<u64, VaultError> {
        let entries = self.entries.read().await;
        Ok(entries.len() as u64)
    }

    async fn last_hash(&self) -> Result<String, VaultError> {
        let hash = self.last_hash.read().await;
        Ok(hash.clone())
    }
}

/// Helper to build a correctly chained audit entry.
///
/// This is the canonical way to create entries for the audit trail.
/// It computes the entry hash and chains to the provided previous hash.
#[allow(clippy::too_many_arguments)]
pub fn build_audit_entry(
    entry_id: String,
    timestamp: u64,
    profile_id: String,
    action: VaultAuditAction,
    model_id: Option<String>,
    tokens_in: Option<u64>,
    tokens_out: Option<u64>,
    latency_ms: u64,
    adapter_id: Option<String>,
    status: VaultAuditStatus,
    error_code: Option<String>,
    previous_hash: String,
) -> VaultAuditEntry {
    let mut entry = VaultAuditEntry {
        entry_id,
        timestamp,
        profile_id,
        action,
        model_id,
        tokens_in,
        tokens_out,
        latency_ms,
        adapter_id,
        status,
        error_code,
        ip_source: "127.0.0.1".to_string(),
        previous_hash,
        entry_hash: String::new(),
        pqc_signature: None,
    };
    // Hash over the fully-populated entry (every field but the two outputs).
    entry.entry_hash = AuditWriter::compute_entry_hash(&entry);
    entry
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_audit_config(tmp: &TempDir) -> AuditConfig {
        AuditConfig {
            db_path: tmp.path().join("audit.json"),
            wal_mode: true,
            sign_interval: 100,
            max_entries: 0,
        }
    }

    fn make_entry(
        id: &str,
        ts: u64,
        profile: &str,
        action: VaultAuditAction,
        prev_hash: &str,
    ) -> VaultAuditEntry {
        build_audit_entry(
            id.to_string(),
            ts,
            profile.to_string(),
            action,
            None,
            None,
            None,
            0,
            None,
            VaultAuditStatus::Success,
            None,
            prev_hash.to_string(),
        )
    }

    #[tokio::test]
    async fn test_append_and_read() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let entry = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&entry).await.unwrap();

        let recent = writer.read_recent(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].entry_id, "e1");
    }

    #[tokio::test]
    async fn test_chain_integrity() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();

        let e2 = make_entry(
            "e2",
            1001,
            "admin",
            VaultAuditAction::ModelLoad,
            &e1.entry_hash,
        );
        writer.append(&e2).await.unwrap();

        let e3 = make_entry(
            "e3",
            1002,
            "user1",
            VaultAuditAction::ModelLoad,
            &e2.entry_hash,
        );
        writer.append(&e3).await.unwrap();

        let verified = writer.verify_chain().await.unwrap();
        assert_eq!(verified, 3);
    }

    #[tokio::test]
    async fn test_broken_chain_detected() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();

        // Try to append with wrong previous hash
        let bad_entry = make_entry(
            "e2",
            1001,
            "admin",
            VaultAuditAction::ModelLoad,
            "wrong_hash",
        );
        let result = writer.append(&bad_entry).await;
        assert!(result.is_err());
    }

    #[test]
    fn entry_hash_covers_previously_unhashed_fields() {
        // Audit H6/V2: the entry hash now binds every security-relevant field, not
        // just previous_hash/timestamp/profile_id/action/status. Editing a field
        // the old 5-field hash ignored must change the digest.
        let base = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::ModelLoad,
            GENESIS_HASH,
        );
        assert_eq!(
            base.entry_hash,
            AuditWriter::compute_entry_hash(&base),
            "the builder's hash must be self-consistent"
        );
        let tampers: [fn(&mut VaultAuditEntry); 7] = [
            |e| e.entry_id = "e-forged".into(),
            |e| e.model_id = Some("swapped-model".into()),
            |e| e.adapter_id = Some("other-adapter".into()),
            |e| e.error_code = Some("E-forged".into()),
            |e| e.ip_source = "10.0.0.9".into(),
            |e| e.tokens_out = Some(999_999),
            |e| e.latency_ms = 42,
        ];
        for tamper in tampers {
            let mut t = base.clone();
            tamper(&mut t);
            assert_ne!(
                t.entry_hash,
                AuditWriter::compute_entry_hash(&t),
                "a tampered formerly-unhashed field must break the entry hash"
            );
        }
    }

    #[tokio::test]
    async fn test_read_by_profile() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();

        let e2 = make_entry(
            "e2",
            1001,
            "user1",
            VaultAuditAction::ModelLoad,
            &e1.entry_hash,
        );
        writer.append(&e2).await.unwrap();

        let e3 = make_entry(
            "e3",
            1002,
            "admin",
            VaultAuditAction::ModelUnload,
            &e2.entry_hash,
        );
        writer.append(&e3).await.unwrap();

        let admin_entries = writer.read_by_profile("admin", 10).await.unwrap();
        assert_eq!(admin_entries.len(), 2);

        let user_entries = writer.read_by_profile("user1", 10).await.unwrap();
        assert_eq!(user_entries.len(), 1);
    }

    #[tokio::test]
    async fn test_read_by_time_range() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();

        let e2 = make_entry(
            "e2",
            2000,
            "admin",
            VaultAuditAction::ModelLoad,
            &e1.entry_hash,
        );
        writer.append(&e2).await.unwrap();

        let e3 = make_entry(
            "e3",
            3000,
            "admin",
            VaultAuditAction::ModelUnload,
            &e2.entry_hash,
        );
        writer.append(&e3).await.unwrap();

        let range = writer.read_by_time_range(1500, 2500).await.unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].entry_id, "e2");
    }

    #[tokio::test]
    async fn test_compliance_export() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();

        let report = writer.export_compliance(0, 9999).await.unwrap();
        assert_eq!(report.total_entries, 1);
        assert!(report.chain_intact);
    }

    #[tokio::test]
    async fn test_checkpoint_signature_written_and_verified() {
        use crate::config::PqcConfig;
        use std::path::PathBuf;
        let tmp = TempDir::new().unwrap();
        let mut cfg = test_audit_config(&tmp);
        cfg.sign_interval = 3; // sign every 3rd entry

        let pqc_cfg = PqcConfig {
            kem_algorithm: "ML-KEM-1024".into(),
            dsa_algorithm: "ML-DSA-87".into(),
            key_store_path: PathBuf::from("/tmp/test-keys"),
            symmetric_cipher: "AES-256-GCM".into(),
        };
        let pqc = std::sync::Arc::new(PqcEngine::new(pqc_cfg));
        pqc.initialize().await.unwrap();

        let writer = AuditWriter::with_pqc(cfg, pqc.clone());
        writer.initialize().await.unwrap();

        let mut prev = GENESIS_HASH.to_string();
        for i in 0..6u64 {
            let entry = make_entry(
                &format!("e{i}"),
                1000 + i,
                "admin",
                VaultAuditAction::SystemStartup,
                &prev,
            );
            prev = entry.entry_hash.clone();
            writer.append(&entry).await.unwrap();
        }

        let all = writer.read_recent(10).await.unwrap();
        // Entries at indices 2 and 5 (1-based 3 and 6) should be signed.
        assert!(all[2].pqc_signature.is_some(), "entry 3 must be signed");
        assert!(all[5].pqc_signature.is_some(), "entry 6 must be signed");
        assert!(all[0].pqc_signature.is_none(), "entry 1 must not be signed");
        assert!(all[1].pqc_signature.is_none(), "entry 2 must not be signed");

        // verify_chain must succeed when checkpoint sigs are valid.
        let verified = writer.verify_chain().await.unwrap();
        assert_eq!(verified, 6);
    }

    #[tokio::test]
    async fn test_tampered_checkpoint_signature_detected() {
        use crate::config::PqcConfig;
        use std::path::PathBuf;
        let tmp = TempDir::new().unwrap();
        let mut cfg = test_audit_config(&tmp);
        cfg.sign_interval = 2;

        let pqc_cfg = PqcConfig {
            kem_algorithm: "ML-KEM-1024".into(),
            dsa_algorithm: "ML-DSA-87".into(),
            key_store_path: PathBuf::from("/tmp/test-keys"),
            symmetric_cipher: "AES-256-GCM".into(),
        };
        let pqc = std::sync::Arc::new(PqcEngine::new(pqc_cfg));
        pqc.initialize().await.unwrap();

        let writer = AuditWriter::with_pqc(cfg, pqc.clone());
        writer.initialize().await.unwrap();

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        writer.append(&e1).await.unwrap();
        let e2 = make_entry(
            "e2",
            1001,
            "admin",
            VaultAuditAction::ModelLoad,
            &e1.entry_hash,
        );
        writer.append(&e2).await.unwrap();

        // Corrupt the signature on the checkpointed entry.
        {
            let mut entries = writer.entries.write().await;
            if let Some(sig) = entries[1].pqc_signature.as_mut() {
                // Flip one hex character to make the signature invalid.
                sig.replace_range(0..2, if &sig[0..2] == "00" { "ff" } else { "00" });
            }
        }

        let result = writer.verify_chain().await;
        assert!(
            matches!(result, Err(VaultError::AuditChainBroken { .. })),
            "tampered checkpoint signature must be detected, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_entry_count_and_last_hash() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        assert_eq!(writer.entry_count().await.unwrap(), 0);
        assert_eq!(writer.last_hash().await.unwrap(), GENESIS_HASH);

        let e1 = make_entry(
            "e1",
            1000,
            "admin",
            VaultAuditAction::SystemStartup,
            GENESIS_HASH,
        );
        let expected_hash = e1.entry_hash.clone();
        writer.append(&e1).await.unwrap();

        assert_eq!(writer.entry_count().await.unwrap(), 1);
        assert_eq!(writer.last_hash().await.unwrap(), expected_hash);
    }
}
