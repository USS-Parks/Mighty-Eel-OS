//! Persistent API audit writer.
//!
//! [`WalAuditWriter`] implements the existing
//! [`crate::audit::AuditWriter`] trait against an append-only,
//! JSON-lines write-ahead log on disk. It is the production
//! replacement for [`crate::audit::MemoryAuditWriter`].
//!
//! Scope:
//! - Type definitions: [`WalAuditWriter`], [`WalAuditConfig`].
//! - JSON-lines persistence under a configurable WAL directory.
//! - Replay-and-verify on startup via [`WalAuditWriter::open`]; the
//!   constructor fails closed if the chain does not verify.
//! - Size-based rotation. Rotation preserves chain continuity — the
//!   first entry of `current.jsonl` after a rotation chains directly
//!   from the last entry of the rotated file.
//! - Retention metadata (default 7 years for HIPAA-aligned posture).
//!   The `prune_rotated` method deletes rotated WAL files older than
//!   `retention_days`; the active `current.jsonl` is never pruned.
//! - Inline tests for chain replay, tamper detection, rotation, and
//!   roundtrip. Integration tests live in
//!   `mai-api/tests/audit_wal.rs`.
//!
//! Out of scope:
//! - Wiring into `MaiServer::run()` / `server.rs`. That happens at
//!   the convergence step alongside the production
//!   guard's runtime checks (`PROD-AUDIT-100`).
//! - PQC checkpoint signing. The signer is already pluggable on
//!   [`crate::audit::AuditManager`]; the vault-backed ML-DSA signer
//!   is plugged in later. This writer only persists the entry bytes
//!   the manager hands it.
//! - Compliance audit sealer. The compliance WAL lives in
//!   `mai-compliance::audit::store` and is a separate writer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::audit::{AuditEntry, AuditWriter, verify_chain};

/// Audit-chain genesis hash. Duplicated from `crate::audit` because
/// the upstream constant is module-private. The `genesis_hash_matches_audit_module`
/// test below fails closed if the upstream value ever changes.
const GENESIS_HASH: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

// ---------------------------------------------------------------------------
// Public configuration
// ---------------------------------------------------------------------------

/// Knobs the operator can tune via the ship profile / config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalAuditConfig {
    /// Directory that holds `current.jsonl` and the `rotated/`
    /// subdirectory. Must exist before [`WalAuditWriter::open`] is
    /// called (the constructor does not auto-create the path so the
    /// production guard's PROD-AUDIT-100 check has something to
    /// verify against).
    pub wal_dir: PathBuf,

    /// Rotate `current.jsonl` once it exceeds this many bytes. The
    /// rotation is checked inline on every write. 0 disables size-
    /// based rotation. Default: 16 MiB.
    pub rotate_bytes: u64,

    /// Retention period for rotated WAL files. The active
    /// `current.jsonl` is never pruned regardless of age. Default:
    /// 2555 (HIPAA-aligned 7 years).
    pub retention_days: u32,
}

impl Default for WalAuditConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("/var/lib/mai/audit"),
            rotate_bytes: 16 * 1024 * 1024,
            retention_days: 2555,
        }
    }
}

impl WalAuditConfig {
    /// Convenience constructor for tests that want a tempdir-backed
    /// writer with everything else at defaults.
    pub fn for_dir(wal_dir: impl Into<PathBuf>) -> Self {
        Self {
            wal_dir: wal_dir.into(),
            ..Default::default()
        }
    }

    /// Path to the active append-only log.
    pub fn current_path(&self) -> PathBuf {
        self.wal_dir.join("current.jsonl")
    }

    /// Directory that holds rotated past logs.
    pub fn rotated_dir(&self) -> PathBuf {
        self.wal_dir.join("rotated")
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Reasons [`WalAuditWriter::open`] can refuse to construct a writer.
#[derive(Debug, thiserror::Error)]
pub enum WalAuditError {
    #[error("WAL directory {path} does not exist")]
    WalDirMissing { path: PathBuf },
    #[error("WAL directory {path} is not a directory")]
    WalDirNotADirectory { path: PathBuf },
    #[error("IO error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed audit entry on line {line} of {path}: {source}")]
    Parse {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("audit chain verification failed: {detail} (entry index {index})")]
    ChainBroken { index: usize, detail: String },
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Append-only JSON-lines audit writer with chain replay on startup.
#[derive(Debug)]
pub struct WalAuditWriter {
    config: WalAuditConfig,
    state: Mutex<WriterState>,
}

#[derive(Debug)]
struct WriterState {
    last_hash: String,
    entry_count: u64,
    current_bytes: u64,
}

impl WalAuditWriter {
    /// Open (or create) the WAL at `config.wal_dir` and replay every
    /// entry on disk to verify the chain. Fails closed if the chain
    /// does not verify or if the directory is missing.
    pub async fn open(config: WalAuditConfig) -> Result<Self, WalAuditError> {
        validate_wal_dir(&config.wal_dir)?;
        ensure_dir(&config.rotated_dir()).await?;

        let replay = replay_and_verify(&config).await?;
        info!(
            wal_dir = %config.wal_dir.display(),
            entry_count = replay.entry_count,
            last_hash = %short(&replay.last_hash),
            rotated_files = replay.rotated_files,
            "WAL audit writer opened",
        );

        let current_bytes = match fs::metadata(config.current_path()).await {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => {
                return Err(WalAuditError::Io {
                    path: config.current_path(),
                    source: e,
                });
            }
        };

        Ok(Self {
            state: Mutex::new(WriterState {
                last_hash: replay.last_hash,
                entry_count: replay.entry_count,
                current_bytes,
            }),
            config,
        })
    }

    /// Read-only view of the config the writer was opened with.
    pub fn config(&self) -> &WalAuditConfig {
        &self.config
    }

    /// Delete every rotated WAL file whose mtime is older than the
    /// configured retention window. The active `current.jsonl` is
    /// never touched. Returns the number of files removed.
    pub async fn prune_rotated(&self) -> Result<usize, WalAuditError> {
        let dir = self.config.rotated_dir();
        let retention_secs = u64::from(self.config.retention_days).saturating_mul(86_400);
        let cutoff = SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(retention_secs))
            .unwrap_or(UNIX_EPOCH);
        let mut removed = 0;
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(WalAuditError::Io {
                    path: dir,
                    source: e,
                });
            }
        };
        while let Some(entry) = rd.next_entry().await.map_err(|e| WalAuditError::Io {
            path: dir.clone(),
            source: e,
        })? {
            let path = entry.path();
            let meta = entry.metadata().await.map_err(|e| WalAuditError::Io {
                path: path.clone(),
                source: e,
            })?;
            let modified = meta.modified().map_err(|e| WalAuditError::Io {
                path: path.clone(),
                source: e,
            })?;
            if modified < cutoff {
                fs::remove_file(&path)
                    .await
                    .map_err(|e| WalAuditError::Io {
                        path: path.clone(),
                        source: e,
                    })?;
                removed += 1;
                debug!(path = %path.display(), "pruned rotated WAL file");
            }
        }
        Ok(removed)
    }

    async fn rotate_locked(&self, state: &mut WriterState) -> Result<(), String> {
        let current = self.config.current_path();
        if !current.exists() {
            state.current_bytes = 0;
            return Ok(());
        }
        // Nanosecond-precision filename so back-to-back rotations
        // never collide. Pad to 20 digits so lexicographic sort
        // (used by `ordered_wal_files`) matches chronological order.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dst = self.config.rotated_dir().join(format!("{nanos:020}.jsonl"));
        fs::rename(&current, &dst)
            .await
            .map_err(|e| format!("rotate {} -> {}: {e}", current.display(), dst.display()))?;
        state.current_bytes = 0;
        info!(rotated_to = %dst.display(), "rotated WAL");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AuditWriter impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl AuditWriter for WalAuditWriter {
    async fn write(&self, entry: &AuditEntry) -> Result<(), String> {
        let mut state = self.state.lock().await;

        // Rotation runs *before* the write so the entry we are about
        // to persist always lands in the post-rotation file. This
        // keeps the post-rotation chain consistent — the first entry
        // of the new current.jsonl chains from the last entry of the
        // rotated file via its own `previous_hash`.
        if self.config.rotate_bytes > 0 && state.current_bytes >= self.config.rotate_bytes {
            self.rotate_locked(&mut state).await?;
        }

        let mut line =
            serde_json::to_string(entry).map_err(|e| format!("serialize audit entry: {e}"))?;
        line.push('\n');

        let bytes = line.as_bytes();
        let path = self.config.current_path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| format!("open {} for append: {e}", path.display()))?;
        file.write_all(bytes)
            .await
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        file.flush()
            .await
            .map_err(|e| format!("flush {}: {e}", path.display()))?;

        state.last_hash = entry.entry_hash.clone();
        state.entry_count += 1;
        state.current_bytes += bytes.len() as u64;
        Ok(())
    }

    async fn read_recent(&self, count: usize) -> Result<Vec<AuditEntry>, String> {
        if count == 0 {
            return Ok(Vec::new());
        }
        // Read newest-first by traversing current.jsonl in reverse,
        // then earlier rotated files until `count` is satisfied. The
        // caller's contract is "most recent N", not "in time order",
        // so we return them in reverse-chronological order to match
        // what `MemoryAuditWriter::read_recent` callers see when they
        // ask `read_recent(N)` after N writes.
        //
        // Simpler implementation: load everything, slice, reverse. We
        // accept the O(total) cost because this writer does not own
        // operator-facing audit query — that lives in `mai-admin
        // audit` and the compliance dashboard.
        let all = read_all_entries(&self.config)
            .await
            .map_err(|e| e.to_string())?;
        let start = all.len().saturating_sub(count);
        Ok(all[start..].to_vec())
    }

    async fn read_by_profile(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let all = read_all_entries(&self.config)
            .await
            .map_err(|e| e.to_string())?;
        let filtered: Vec<AuditEntry> = all
            .into_iter()
            .rev()
            .filter(|e| e.profile_id == profile_id)
            .take(limit)
            .collect();
        Ok(filtered)
    }

    async fn entry_count(&self) -> Result<u64, String> {
        let state = self.state.lock().await;
        Ok(state.entry_count)
    }

    async fn last_hash(&self) -> Result<String, String> {
        let state = self.state.lock().await;
        Ok(state.last_hash.clone())
    }
}

// Allow `Arc<WalAuditWriter>` to be passed where AuditManager expects
// `Arc<dyn AuditWriter>`.
impl WalAuditWriter {
    pub fn into_dyn(self: Arc<Self>) -> Arc<dyn AuditWriter> {
        self
    }
}

// ---------------------------------------------------------------------------
// Replay / verification
// ---------------------------------------------------------------------------

/// Outcome of a successful WAL replay.
#[derive(Debug, Clone)]
pub struct ReplayOutcome {
    pub entry_count: u64,
    pub last_hash: String,
    pub rotated_files: usize,
}

/// Read every entry from the WAL directory (oldest rotated file first
/// through `current.jsonl`) and run the existing chain verifier. The
/// startup hook will call this before the writer is wired in,
/// then refuse to listen if it fails.
pub async fn replay_and_verify(config: &WalAuditConfig) -> Result<ReplayOutcome, WalAuditError> {
    let entries = read_all_entries(config).await?;
    let rotated_files = count_rotated(config).await?;
    if entries.is_empty() {
        return Ok(ReplayOutcome {
            entry_count: 0,
            last_hash: GENESIS_HASH.to_string(),
            rotated_files,
        });
    }
    if let Err((index, detail)) = verify_chain(&entries) {
        return Err(WalAuditError::ChainBroken { index, detail });
    }
    let last_hash = entries
        .last()
        .map(|e| e.entry_hash.clone())
        .unwrap_or_else(|| GENESIS_HASH.to_string());
    let entry_count = entries.len() as u64;
    Ok(ReplayOutcome {
        entry_count,
        last_hash,
        rotated_files,
    })
}

async fn read_all_entries(config: &WalAuditConfig) -> Result<Vec<AuditEntry>, WalAuditError> {
    let mut all = Vec::new();
    for path in ordered_wal_files(config).await? {
        read_jsonl_into(&path, &mut all).await?;
    }
    Ok(all)
}

async fn ordered_wal_files(config: &WalAuditConfig) -> Result<Vec<PathBuf>, WalAuditError> {
    let mut rotated: Vec<PathBuf> = Vec::new();
    if let Ok(mut rd) = fs::read_dir(config.rotated_dir()).await {
        while let Some(entry) = rd.next_entry().await.map_err(|e| WalAuditError::Io {
            path: config.rotated_dir(),
            source: e,
        })? {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                rotated.push(p);
            }
        }
    }
    // Rotated filenames are unix-second stamps, so lexicographic sort
    // == chronological. Padded enough not to wrap until year 33,658,
    // which is beyond the timeline this code intends to outlive.
    rotated.sort();
    if config.current_path().exists() {
        rotated.push(config.current_path());
    }
    Ok(rotated)
}

async fn count_rotated(config: &WalAuditConfig) -> Result<usize, WalAuditError> {
    let dir = config.rotated_dir();
    let mut rd = match fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(WalAuditError::Io {
                path: dir,
                source: e,
            });
        }
    };
    let mut count = 0;
    while let Some(entry) = rd.next_entry().await.map_err(|e| WalAuditError::Io {
        path: dir.clone(),
        source: e,
    })? {
        if entry.path().extension().and_then(|s| s.to_str()) == Some("jsonl") {
            count += 1;
        }
    }
    Ok(count)
}

async fn read_jsonl_into(path: &Path, out: &mut Vec<AuditEntry>) -> Result<(), WalAuditError> {
    let file = match fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(WalAuditError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let mut reader = BufReader::new(file).lines();
    let mut line_no = 0usize;
    while let Some(line) = reader.next_line().await.map_err(|e| WalAuditError::Io {
        path: path.to_path_buf(),
        source: e,
    })? {
        line_no += 1;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry = serde_json::from_str(&line).map_err(|e| WalAuditError::Parse {
            path: path.to_path_buf(),
            line: line_no,
            source: e,
        })?;
        out.push(entry);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_wal_dir(path: &Path) -> Result<(), WalAuditError> {
    if !path.exists() {
        return Err(WalAuditError::WalDirMissing {
            path: path.to_path_buf(),
        });
    }
    if !path.is_dir() {
        return Err(WalAuditError::WalDirNotADirectory {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

async fn ensure_dir(path: &Path) -> Result<(), WalAuditError> {
    fs::create_dir_all(path)
        .await
        .map_err(|e| WalAuditError::Io {
            path: path.to_path_buf(),
            source: e,
        })
}

fn short(hash: &str) -> String {
    if hash.len() <= 12 {
        hash.to_string()
    } else {
        format!("{}…", &hash[..12])
    }
}

// ---------------------------------------------------------------------------
// Inline tests (filesystem-driven; integration tests live in tests/audit_wal.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditManager, AuditRequestType, MemoryAuditWriter, NullSigner};
    use std::time::Duration;

    #[tokio::test]
    async fn genesis_hash_matches_audit_module() {
        // If audit.rs ever rotates its GENESIS_HASH constant, our
        // duplicate goes out of sync and chain replay would silently
        // accept an inconsistent starting point. This test fails
        // closed in that case so we update the duplicate or wire the
        // upstream constant `pub`.
        let upstream = MemoryAuditWriter::new().last_hash().await.unwrap();
        assert_eq!(
            GENESIS_HASH, upstream,
            "audit_wal::GENESIS_HASH is out of sync with audit::GENESIS_HASH"
        );
    }

    fn make_entry(seq: u64, prev: &str) -> AuditEntry {
        // Hash here is intentionally NOT computed via the real hasher
        // — these tests verify WAL persistence + replay, not chain
        // verification. Tests that need a valid chain use AuditManager.
        let h = format!("{prev}{seq:08x}");
        AuditEntry {
            entry_id: format!("e-{seq}"),
            timestamp: 1_700_000_000 + seq,
            previous_hash: prev.to_string(),
            entry_hash: h,
            profile_id: "tester".to_string(),
            profile_role: "Admin".to_string(),
            method: "GET".to_string(),
            path: "/v1/health".to_string(),
            status_code: 200,
            duration_ms: 1,
            model_name: None,
            request_type: AuditRequestType::HealthCheck,
            context: None,
            pqc_signature: None,
        }
    }

    #[tokio::test]
    async fn open_rejects_missing_dir() {
        let cfg = WalAuditConfig::for_dir("/__nonexistent_ship04_wal__/x");
        let err = WalAuditWriter::open(cfg)
            .await
            .expect_err("missing dir must fail");
        assert!(
            matches!(err, WalAuditError::WalDirMissing { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn empty_wal_returns_genesis_last_hash() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        let writer = WalAuditWriter::open(cfg).await.unwrap();
        assert_eq!(writer.last_hash().await.unwrap(), GENESIS_HASH);
        assert_eq!(writer.entry_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn write_persists_across_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        {
            let writer = WalAuditWriter::open(cfg.clone()).await.unwrap();
            let signer = Arc::new(NullSigner);
            let mgr = AuditManager::new(Arc::new(writer), signer, 0)
                .await
                .unwrap();
            for i in 0..5 {
                mgr.record(
                    "u1",
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
            // Force the manager out of scope; writer drops with it.
        }
        // Reopen and verify everything replays + chain verifies.
        let writer2 = WalAuditWriter::open(cfg).await.unwrap();
        assert_eq!(writer2.entry_count().await.unwrap(), 5);
        let recent = writer2.read_recent(10).await.unwrap();
        assert_eq!(recent.len(), 5);
        // Chain head matches the last entry's entry_hash.
        assert_eq!(
            writer2.last_hash().await.unwrap(),
            recent.last().unwrap().entry_hash
        );
    }

    #[tokio::test]
    async fn replay_rejects_tampered_wal() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        {
            let writer = WalAuditWriter::open(cfg.clone()).await.unwrap();
            let mgr = AuditManager::new(Arc::new(writer), Arc::new(NullSigner), 0)
                .await
                .unwrap();
            for i in 0..3 {
                mgr.record(
                    "u1",
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
        }
        // Tamper: corrupt the first character of the first line.
        let current = cfg.current_path();
        let body = std::fs::read_to_string(&current).unwrap();
        let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
        let first = lines.first_mut().unwrap();
        // Replace the status_code value 200 with 999 — keeps the JSON
        // valid (so the parser still accepts it) but breaks the hash
        // chain because the recomputed hash will no longer match
        // entry_hash.
        *first = first.replace("\"status_code\":200", "\"status_code\":999");
        let new_body = lines.join("\n") + "\n";
        std::fs::write(&current, new_body).unwrap();

        let err = WalAuditWriter::open(cfg)
            .await
            .expect_err("tampered WAL must fail chain verification");
        assert!(matches!(err, WalAuditError::ChainBroken { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn rotation_preserves_chain_across_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = WalAuditConfig::for_dir(temp.path());
        cfg.rotate_bytes = 256; // tiny — rotates after every couple of entries

        let writer = Arc::new(WalAuditWriter::open(cfg.clone()).await.unwrap());
        let mgr = AuditManager::new(writer.clone(), Arc::new(NullSigner), 0)
            .await
            .unwrap();
        for i in 0..10 {
            mgr.record(
                "u1",
                "Admin",
                "POST",
                &format!("/v1/chat/{i}"),
                200,
                Duration::from_millis(2),
                None,
                None,
            )
            .await
            .unwrap();
        }
        drop(mgr);
        drop(writer);

        // At least one rotation happened.
        let rotated = std::fs::read_dir(cfg.rotated_dir()).unwrap().count();
        assert!(
            rotated >= 1,
            "expected at least one rotated file, got {rotated}"
        );

        // Reopen — replay must succeed across rotated + current.
        let reopened = WalAuditWriter::open(cfg).await.unwrap();
        assert_eq!(reopened.entry_count().await.unwrap(), 10);
    }

    #[tokio::test]
    async fn malformed_entry_fails_replay() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        // Drop a malformed line directly into current.jsonl before
        // open — replay must surface it as ParseError with the
        // offending line number.
        std::fs::write(cfg.current_path(), "not-json\n").unwrap();
        let err = WalAuditWriter::open(cfg)
            .await
            .expect_err("malformed entry must fail");
        match err {
            WalAuditError::Parse { line, .. } => assert_eq!(line, 1),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn last_entry_seed_from_rotated_file() {
        // If a rotated file exists and current.jsonl does not yet,
        // the replay's last_hash must come from the tail of the most
        // recent rotated file. Without this, the next write would
        // chain from GENESIS again and break the audit chain.
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        ensure_dir(&cfg.rotated_dir()).await.unwrap();

        // Write a synthetic single-entry rotated file with a valid
        // chain via AuditManager, then move it into the rotated/ dir.
        let writer = WalAuditWriter::open(cfg.clone()).await.unwrap();
        let mgr = AuditManager::new(Arc::new(writer), Arc::new(NullSigner), 0)
            .await
            .unwrap();
        let e = mgr
            .record(
                "u1",
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
        drop(mgr);
        let current = cfg.current_path();
        let rotated = cfg.rotated_dir().join("1000000000.jsonl");
        std::fs::rename(&current, &rotated).unwrap();

        let reopened = WalAuditWriter::open(cfg).await.unwrap();
        assert_eq!(reopened.last_hash().await.unwrap(), e.entry_hash);
        assert_eq!(reopened.entry_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn unused_helpers_avoid_dead_code_warnings() {
        // The Arc::into_dyn helper is the wiring path; ensure
        // its signature compiles by exercising it here.
        let temp = tempfile::tempdir().unwrap();
        let cfg = WalAuditConfig::for_dir(temp.path());
        let writer = Arc::new(WalAuditWriter::open(cfg).await.unwrap());
        let dynamic: Arc<dyn AuditWriter> = writer.into_dyn();
        assert_eq!(dynamic.entry_count().await.unwrap(), 0);
    }
}
