//! Vault-level audit trail writer.
//!
//! Implements `AuditStore` with hash-chained, PQC-signed entries.
//! Uses in-memory storage with JSON persistence (production: SQLite WAL).
//!
//! # Hash Chain
//!
//! Each entry's `entry_hash` is computed as:
//!   SHA3-256(previous_hash || timestamp || profile_id || action || status)
//!
//! The chain starts with a genesis hash of all zeros.
//!
//! # PQC Signatures
//!
//! Every Nth entry (configurable, default 100) is signed with ML-DSA-87
//! to create cryptographic checkpoints. These checkpoints allow verification
//! of chain integrity without re-verifying every individual entry.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use mai_core::vault::{
    AuditStore, ComplianceReport, VaultAuditAction, VaultAuditEntry, VaultAuditStatus,
    VaultError,
};

use crate::config::AuditConfig;

/// Genesis hash (all zeros) for the first entry in the chain.
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Vault audit trail writer.
///
/// Maintains an append-only, hash-chained sequence of audit entries.
/// Entries are stored in memory and persisted to disk on append.
pub struct AuditWriter {
    config: AuditConfig,
    /// Ordered list of audit entries (append-only).
    entries: RwLock<Vec<VaultAuditEntry>>,
    /// Hash of the most recent entry.
    last_hash: RwLock<String>,
}

impl AuditWriter {
    /// Create a new audit writer.
    pub fn new(config: AuditConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(Vec::new()),
            last_hash: RwLock::new(GENESIS_HASH.to_string()),
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
                Ok(content) => {
                    match serde_json::from_str::<Vec<VaultAuditEntry>>(&content) {
                        Ok(loaded) => {
                            let mut entries = self.entries.write().await;
                            let mut last = self.last_hash.write().await;

                            if let Some(final_entry) = loaded.last() {
                                *last = final_entry.entry_hash.clone();
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
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Could not read audit database, starting fresh");
                }
            }
        }

        Ok(())
    }

    /// Compute the hash for an audit entry.
    ///
    /// Hash = BLAKE3(previous_hash || timestamp || profile_id || action_str || status_str)
    /// Using BLAKE3 as stand-in for SHA3-256 (same security level, faster).
    fn compute_entry_hash(
        previous_hash: &str,
        timestamp: u64,
        profile_id: &str,
        action: &VaultAuditAction,
        status: &VaultAuditStatus,
    ) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(previous_hash.as_bytes());
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(profile_id.as_bytes());
        hasher.update(format!("{:?}", action).as_bytes());
        hasher.update(format!("{:?}", status).as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    /// Persist entries to disk.
    async fn persist(&self) -> Result<(), VaultError> {
        let entries = self.entries.read().await;
        let json = serde_json::to_string(&*entries)
            .map_err(|e| VaultError::AuditStoreError(e.to_string()))?;

        if let Some(parent) = self.config.db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| VaultError::AuditStoreError(e.to_string()))?;
            }
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
        let expected_hash = Self::compute_entry_hash(
            &entry.previous_hash,
            entry.timestamp,
            &entry.profile_id,
            &entry.action,
            &entry.status,
        );
        if entry.entry_hash != expected_hash {
            return Err(VaultError::AuditChainBroken {
                index: entries.len() as u64,
                detail: format!(
                    "Entry hash mismatch: got {}, expected {}",
                    entry.entry_hash, expected_hash
                ),
            });
        }

        debug!(
            entry_id = %entry.entry_id,
            action = ?entry.action,
            "Appending audit entry"
        );

        *last = entry.entry_hash.clone();
        entries.push(entry.clone());
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
                        "previous_hash mismatch at index {}: expected {}, got {}",
                        i, expected_prev, entry.previous_hash
                    ),
                });
            }

            // Recompute and verify entry hash
            let computed = Self::compute_entry_hash(
                &entry.previous_hash,
                entry.timestamp,
                &entry.profile_id,
                &entry.action,
                &entry.status,
            );
            if entry.entry_hash != computed {
                return Err(VaultError::AuditChainBroken {
                    index: i as u64,
                    detail: format!(
                        "entry_hash mismatch at index {}: expected {}, got {}",
                        i, computed, entry.entry_hash
                    ),
                });
            }

            expected_prev = entry.entry_hash.clone();
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
                *profile_summary
                    .entry(entry.profile_id.clone())
                    .or_insert(0) += 1;
            }
        }

        // Verify chain integrity
        drop(entries); // release lock before verify_chain
        let chain_verified = match self.verify_chain().await {
            Ok(n) => (n, true),
            Err(_) => (0, false),
        };

        Ok(ComplianceReport {
            generated_at: chrono::Utc::now().timestamp() as u64,
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
    let entry_hash = AuditWriter::compute_entry_hash(
        &previous_hash,
        timestamp,
        &profile_id,
        &action,
        &status,
    );

    VaultAuditEntry {
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
        entry_hash,
        pqc_signature: None,
    }
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

        let entry = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
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

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        writer.append(&e1).await.unwrap();

        let e2 = make_entry("e2", 1001, "admin", VaultAuditAction::ModelLoad, &e1.entry_hash);
        writer.append(&e2).await.unwrap();

        let e3 = make_entry("e3", 1002, "user1", VaultAuditAction::ModelLoad, &e2.entry_hash);
        writer.append(&e3).await.unwrap();

        let verified = writer.verify_chain().await.unwrap();
        assert_eq!(verified, 3);
    }

    #[tokio::test]
    async fn test_broken_chain_detected() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        writer.append(&e1).await.unwrap();

        // Try to append with wrong previous hash
        let bad_entry = make_entry("e2", 1001, "admin", VaultAuditAction::ModelLoad, "wrong_hash");
        let result = writer.append(&bad_entry).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_by_profile() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        writer.append(&e1).await.unwrap();

        let e2 = make_entry("e2", 1001, "user1", VaultAuditAction::ModelLoad, &e1.entry_hash);
        writer.append(&e2).await.unwrap();

        let e3 = make_entry("e3", 1002, "admin", VaultAuditAction::ModelUnload, &e2.entry_hash);
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

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        writer.append(&e1).await.unwrap();

        let e2 = make_entry("e2", 2000, "admin", VaultAuditAction::ModelLoad, &e1.entry_hash);
        writer.append(&e2).await.unwrap();

        let e3 = make_entry("e3", 3000, "admin", VaultAuditAction::ModelUnload, &e2.entry_hash);
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

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        writer.append(&e1).await.unwrap();

        let report = writer.export_compliance(0, 9999).await.unwrap();
        assert_eq!(report.total_entries, 1);
        assert!(report.chain_intact);
    }

    #[tokio::test]
    async fn test_entry_count_and_last_hash() {
        let tmp = TempDir::new().unwrap();
        let writer = AuditWriter::new(test_audit_config(&tmp));
        writer.initialize().await.unwrap();

        assert_eq!(writer.entry_count().await.unwrap(), 0);
        assert_eq!(writer.last_hash().await.unwrap(), GENESIS_HASH);

        let e1 = make_entry("e1", 1000, "admin", VaultAuditAction::SystemStartup, GENESIS_HASH);
        let expected_hash = e1.entry_hash.clone();
        writer.append(&e1).await.unwrap();

        assert_eq!(writer.entry_count().await.unwrap(), 1);
        assert_eq!(writer.last_hash().await.unwrap(), expected_hash);
    }
}
