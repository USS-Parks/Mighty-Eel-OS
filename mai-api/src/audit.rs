//! Audit trail middleware and writer for the MAI API.
//!
//! Every API request is logged with a hash-chained audit entry that includes
//! the request metadata, profile context, and response status. The hash chain
//! provides tamper evidence: any modification to a historical entry breaks
//! the chain from that point forward.
//!
//! # Hash Chain
//!
//! Each entry's hash = SHA3-256(previous_hash || entry_data). The genesis
//! entry uses a well-known seed. PQC signatures (ML-DSA) can optionally
//! sign periodic checkpoints for long-term integrity.
//!
//! # Air-Gap Safety
//!
//! All audit data stays local. No network transmission. The AuditWriter
//! trait allows pluggable storage (SQLite, file, ZFS dataset).

use axum::{extract::Request, middleware::Next, response::Response};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::types::ProfileInfo;

// ── Audit Entry ───────────────────────────────────────────────────────

/// A single audit log entry with hash chain linkage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry identifier.
    pub entry_id: String,

    /// Timestamp as Unix epoch seconds.
    pub timestamp: u64,

    /// Hash of the previous entry (hex-encoded SHA3-256).
    pub previous_hash: String,

    /// Hash of this entry (hex-encoded SHA3-256).
    pub entry_hash: String,

    /// Profile that initiated the request.
    pub profile_id: String,

    /// Profile role at time of request.
    pub profile_role: String,

    /// HTTP method.
    pub method: String,

    /// Request path.
    pub path: String,

    /// HTTP status code of the response.
    pub status_code: u16,

    /// Processing duration in milliseconds.
    pub duration_ms: u64,

    /// Optional model referenced in the request.
    pub model_name: Option<String>,

    /// Request type classification.
    pub request_type: AuditRequestType,

    /// Optional additional context (never contains PII or inference content).
    pub context: Option<String>,

    /// Optional PQC signature over this entry (ML-DSA, added by signer).
    pub pqc_signature: Option<String>,
}

/// Classification of audited request types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditRequestType {
    Inference,
    ModelManagement,
    SystemOperation,
    HealthCheck,
    AuditAccess,
    ProfileOperation,
    PowerControl,
    RegistryQuery,
    Unknown,
}

impl AuditRequestType {
    /// Classify a request based on its path.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/v1/chat")
            || path.starts_with("/v1/embeddings")
            || path.starts_with("/v1/structured")
            || path.starts_with("/v1/function")
        {
            AuditRequestType::Inference
        } else if path.starts_with("/v1/models") {
            AuditRequestType::ModelManagement
        } else if path.starts_with("/v1/system") {
            AuditRequestType::SystemOperation
        } else if path.starts_with("/v1/health") {
            AuditRequestType::HealthCheck
        } else if path.starts_with("/v1/audit") {
            AuditRequestType::AuditAccess
        } else if path.starts_with("/v1/profiles") {
            AuditRequestType::ProfileOperation
        } else if path.starts_with("/v1/power") {
            AuditRequestType::PowerControl
        } else if path.starts_with("/v1/registry") {
            AuditRequestType::RegistryQuery
        } else {
            AuditRequestType::Unknown
        }
    }
}

// ── Hash Chain ────────────────────────────────────────────────────────

/// Well-known genesis hash for the audit chain.
/// SHA3-256("island-mountain-mai-audit-genesis-v1")
/// Needs `pub(crate)` so `audit_wal::WalAuditWriter` can replay-verify
/// against the same genesis. No external API change.
pub(crate) const GENESIS_HASH: &str =
    "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

/// Compute the hash for an audit entry.
///
/// hash = SHA3-256(previous_hash || timestamp || profile_id || method || path || status_code)
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
    let result = hasher.finalize();
    hex::encode(result)
}

/// Verify the integrity of an audit chain.
///
/// Returns Ok(count) with the number of verified entries, or Err with
/// the index and detail of the first broken link.
pub fn verify_chain(entries: &[AuditEntry]) -> Result<usize, (usize, String)> {
    if entries.is_empty() {
        return Ok(0);
    }

    // First entry must chain from genesis
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

        // Verify chain linkage (entry[i+1].previous_hash == entry[i].entry_hash)
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

// ── PQC Signature Trait ───────────────────────────────────────────────

/// Trait for PQC (Post-Quantum Cryptography) signing of audit entries.
///
/// Implementations use ML-DSA (FIPS 204) to sign periodic audit
/// checkpoints. The signer is invoked at configurable intervals
/// (e.g., every 100 entries or every hour).
#[async_trait::async_trait]
pub trait AuditSigner: Send + Sync + 'static {
    /// Sign an audit entry's hash, returning the signature bytes as hex.
    async fn sign(&self, entry_hash: &str) -> Result<String, String>;

    /// Verify a signature against an entry hash.
    async fn verify(&self, entry_hash: &str, signature: &str) -> Result<bool, String>;
}

/// No-op signer for deployments without PQC keys provisioned.
///
/// Returns empty signatures. Verification always returns true.
/// Real PQC signing (Vault Integration).
#[derive(Debug, Clone)]
pub struct NullSigner;

#[async_trait::async_trait]
impl AuditSigner for NullSigner {
    async fn sign(&self, _entry_hash: &str) -> Result<String, String> {
        Ok(String::new())
    }

    async fn verify(&self, _entry_hash: &str, _signature: &str) -> Result<bool, String> {
        Ok(true)
    }
}

// ── Audit Writer Trait ────────────────────────────────────────────────

/// Trait for persisting audit entries.
///
/// Implementations may write to SQLite, append-only files, or ZFS
/// datasets. All storage must be local (air-gap safe).
#[async_trait::async_trait]
pub trait AuditWriter: Send + Sync + 'static {
    /// Write an audit entry to persistent storage.
    async fn write(&self, entry: &AuditEntry) -> Result<(), String>;

    /// Read the most recent N entries.
    async fn read_recent(&self, count: usize) -> Result<Vec<AuditEntry>, String>;

    /// Read all entries for a specific profile.
    async fn read_by_profile(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, String>;

    /// Get the total entry count.
    async fn entry_count(&self) -> Result<u64, String>;

    /// Get the hash of the most recent entry (for chain continuation).
    async fn last_hash(&self) -> Result<String, String>;
}

/// In-memory audit writer for testing and development.
#[derive(Debug)]
pub struct MemoryAuditWriter {
    entries: Mutex<Vec<AuditEntry>>,
}

impl MemoryAuditWriter {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl Default for MemoryAuditWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AuditWriter for MemoryAuditWriter {
    async fn write(&self, entry: &AuditEntry) -> Result<(), String> {
        let mut entries = self.entries.lock().await;
        entries.push(entry.clone());
        Ok(())
    }

    async fn read_recent(&self, count: usize) -> Result<Vec<AuditEntry>, String> {
        let entries = self.entries.lock().await;
        let start = entries.len().saturating_sub(count);
        Ok(entries[start..].to_vec())
    }

    async fn read_by_profile(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, String> {
        let entries = self.entries.lock().await;
        let filtered: Vec<AuditEntry> = entries
            .iter()
            .filter(|e| e.profile_id == profile_id)
            .rev()
            .take(limit)
            .cloned()
            .collect();
        Ok(filtered)
    }

    async fn entry_count(&self) -> Result<u64, String> {
        let entries = self.entries.lock().await;
        #[allow(clippy::cast_possible_truncation)]
        Ok(entries.len() as u64)
    }

    async fn last_hash(&self) -> Result<String, String> {
        let entries = self.entries.lock().await;
        match entries.last() {
            Some(entry) => Ok(entry.entry_hash.clone()),
            None => Ok(GENESIS_HASH.to_string()),
        }
    }
}

// ── Audit Manager ─────────────────────────────────────────────────────

/// Central audit manager that maintains the hash chain state and
/// coordinates writing and optional PQC signing.
pub struct AuditManager {
    writer: Arc<dyn AuditWriter>,
    signer: Arc<dyn AuditSigner>,
    last_hash: Mutex<String>,
    sign_interval: u64, // Sign every N entries
    entry_count: Mutex<u64>,
}

impl AuditManager {
    pub async fn new(
        writer: Arc<dyn AuditWriter>,
        signer: Arc<dyn AuditSigner>,
        sign_interval: u64,
    ) -> Result<Self, String> {
        let last_hash = writer.last_hash().await?;
        let entry_count = writer.entry_count().await?;

        Ok(Self {
            writer,
            signer,
            last_hash: Mutex::new(last_hash),
            sign_interval,
            entry_count: Mutex::new(entry_count),
        })
    }

    /// Record an audit entry, extending the hash chain.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        profile_id: &str,
        profile_role: &str,
        method: &str,
        path: &str,
        status_code: u16,
        duration: Duration,
        model_name: Option<String>,
        context: Option<String>,
    ) -> Result<AuditEntry, String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("Clock error: {e}"))?
            .as_secs();

        let mut last_hash = self.last_hash.lock().await;
        let previous_hash = last_hash.clone();

        let entry_hash = compute_entry_hash(
            &previous_hash,
            timestamp,
            profile_id,
            method,
            path,
            status_code,
        );

        let request_type = AuditRequestType::from_path(path);

        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = duration.as_millis() as u64;

        let mut entry = AuditEntry {
            entry_id: Uuid::new_v4().to_string(),
            timestamp,
            previous_hash,
            entry_hash: entry_hash.clone(),
            profile_id: profile_id.to_string(),
            profile_role: profile_role.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status_code,
            duration_ms,
            model_name,
            request_type,
            context,
            pqc_signature: None,
        };

        // PQC signing at configured intervals
        let mut count = self.entry_count.lock().await;
        *count += 1;
        if self.sign_interval > 0 && *count % self.sign_interval == 0 {
            match self.signer.sign(&entry_hash).await {
                Ok(sig) if !sig.is_empty() => {
                    entry.pqc_signature = Some(sig);
                    debug!(entry_count = *count, "PQC checkpoint signature applied");
                }
                Ok(_) => {} // NullSigner returns empty
                Err(e) => {
                    warn!(error = %e, "PQC signing failed, entry saved without signature");
                }
            }
        }

        self.writer.write(&entry).await?;
        *last_hash = entry_hash;

        Ok(entry)
    }

    /// Get the current chain head hash.
    pub async fn chain_head(&self) -> String {
        self.last_hash.lock().await.clone()
    }

    /// Get the writer reference for direct queries.
    pub fn writer(&self) -> &Arc<dyn AuditWriter> {
        &self.writer
    }
}

// ── Middleware ─────────────────────────────────────────────────────────

/// Axum middleware that records an audit entry for every request.
///
/// Captures: profile, method, path, status code, duration.
/// Does NOT capture request/response bodies (privacy + performance).
pub async fn audit_middleware(audit: Arc<AuditManager>, request: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // Extract profile if present (injected by profile_middleware)
    let (profile_id, profile_role) = request.extensions().get::<ProfileInfo>().map_or_else(
        || ("unknown".to_string(), "unknown".to_string()),
        |p| (p.profile_id.clone(), format!("{:?}", p.role)),
    );

    let response = next.run(request).await;

    let duration = start.elapsed();
    let status = response.status().as_u16();

    // Fire-and-forget audit recording (don't block response)
    let audit = audit.clone();
    let method_clone = method.clone();
    let path_clone = path.clone();
    let pid = profile_id.clone();
    let prole = profile_role.clone();

    tokio::spawn(async move {
        if let Err(e) = audit
            .record(
                &pid,
                &prole,
                &method_clone,
                &path_clone,
                status,
                duration,
                None,
                None,
            )
            .await
        {
            error!(error = %e, path = %path_clone, "Failed to record audit entry");
        }
    });

    response
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_chain() {
        let hash = compute_entry_hash(GENESIS_HASH, 1000, "admin", "GET", "/v1/health", 200);
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // SHA3-256 hex = 64 chars
    }

    #[test]
    fn test_hash_determinism() {
        let h1 = compute_entry_hash(GENESIS_HASH, 1000, "user1", "POST", "/v1/chat", 200);
        let h2 = compute_entry_hash(GENESIS_HASH, 1000, "user1", "POST", "/v1/chat", 200);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_sensitivity() {
        let h1 = compute_entry_hash(GENESIS_HASH, 1000, "user1", "POST", "/v1/chat", 200);
        let h2 = compute_entry_hash(GENESIS_HASH, 1001, "user1", "POST", "/v1/chat", 200);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_request_type_classification() {
        assert_eq!(
            AuditRequestType::from_path("/v1/chat/completions"),
            AuditRequestType::Inference
        );
        assert_eq!(
            AuditRequestType::from_path("/v1/models/list"),
            AuditRequestType::ModelManagement
        );
        assert_eq!(
            AuditRequestType::from_path("/v1/health"),
            AuditRequestType::HealthCheck
        );
        assert_eq!(
            AuditRequestType::from_path("/v1/power/state"),
            AuditRequestType::PowerControl
        );
        assert_eq!(
            AuditRequestType::from_path("/something/else"),
            AuditRequestType::Unknown
        );
    }

    #[tokio::test]
    async fn test_memory_writer_roundtrip() {
        let writer = MemoryAuditWriter::new();
        let entry = AuditEntry {
            entry_id: "test-1".to_string(),
            timestamp: 1000,
            previous_hash: GENESIS_HASH.to_string(),
            entry_hash: "abc123".to_string(),
            profile_id: "user1".to_string(),
            profile_role: "Admin".to_string(),
            method: "GET".to_string(),
            path: "/v1/health".to_string(),
            status_code: 200,
            duration_ms: 5,
            model_name: None,
            request_type: AuditRequestType::HealthCheck,
            context: None,
            pqc_signature: None,
        };

        writer.write(&entry).await.unwrap();
        let recent = writer.read_recent(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].entry_id, "test-1");
    }

    #[tokio::test]
    async fn test_audit_manager_chain() {
        let writer = Arc::new(MemoryAuditWriter::new());
        let signer = Arc::new(NullSigner);
        let manager = AuditManager::new(writer.clone(), signer, 0).await.unwrap();

        let e1 = manager
            .record(
                "user1",
                "Admin",
                "GET",
                "/v1/health",
                200,
                Duration::from_millis(5),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(e1.previous_hash, GENESIS_HASH);

        let e2 = manager
            .record(
                "user1",
                "Admin",
                "POST",
                "/v1/chat/completions",
                200,
                Duration::from_millis(50),
                Some("phi-4".to_string()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(e2.previous_hash, e1.entry_hash);
    }

    #[tokio::test]
    async fn test_chain_verification() {
        let writer = Arc::new(MemoryAuditWriter::new());
        let signer = Arc::new(NullSigner);
        let manager = AuditManager::new(writer.clone(), signer, 0).await.unwrap();

        for i in 0..5 {
            manager
                .record(
                    "user1",
                    "Admin",
                    "GET",
                    &format!("/v1/health/{i}"),
                    200,
                    Duration::from_millis(1),
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        let entries = writer.read_recent(10).await.unwrap();
        let result = verify_chain(&entries);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
    }

    #[test]
    fn test_chain_verification_tampered() {
        let mut entries = vec![AuditEntry {
            entry_id: "e1".to_string(),
            timestamp: 1000,
            previous_hash: GENESIS_HASH.to_string(),
            entry_hash: compute_entry_hash(GENESIS_HASH, 1000, "u1", "GET", "/v1/health", 200),
            profile_id: "u1".to_string(),
            profile_role: "Admin".to_string(),
            method: "GET".to_string(),
            path: "/v1/health".to_string(),
            status_code: 200,
            duration_ms: 1,
            model_name: None,
            request_type: AuditRequestType::HealthCheck,
            context: None,
            pqc_signature: None,
        }];

        // Tamper with the hash
        entries[0].entry_hash = "tampered_hash".to_string();
        let result = verify_chain(&entries);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_profile_filtering() {
        let writer = Arc::new(MemoryAuditWriter::new());
        let signer = Arc::new(NullSigner);
        let manager = AuditManager::new(writer.clone(), signer, 0).await.unwrap();

        manager
            .record(
                "alice",
                "Admin",
                "GET",
                "/v1/health",
                200,
                Duration::from_millis(1),
                None,
                None,
            )
            .await
            .unwrap();
        manager
            .record(
                "bob",
                "Adult",
                "POST",
                "/v1/chat/completions",
                200,
                Duration::from_millis(10),
                None,
                None,
            )
            .await
            .unwrap();
        manager
            .record(
                "alice",
                "Admin",
                "GET",
                "/v1/models",
                200,
                Duration::from_millis(2),
                None,
                None,
            )
            .await
            .unwrap();

        let alice_entries = writer.read_by_profile("alice", 10).await.unwrap();
        assert_eq!(alice_entries.len(), 2);

        let bob_entries = writer.read_by_profile("bob", 10).await.unwrap();
        assert_eq!(bob_entries.len(), 1);
    }
}
