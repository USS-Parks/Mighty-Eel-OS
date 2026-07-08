//! Append-only audit store.
//!
//! [`AuditStore`] keeps the running tail of finalised audit entries
//! in memory and (optionally) mirrors writes to an append-only WAL
//! file on disk. It supports the offline-queue / replay
//! semantics: callers can mark the store offline, accumulate
//! correlation events, then call [`AuditStore::drain_offline_queue`]
//! once connectivity returns.
//!
//! Storage encryption is left as a hook ([`StoreSealer`]). The
//! shipped [`NullSealer`] is the bring-up default; production
//! deployments plug in a vault-backed AEAD sealer (vault
//! integration). The hook is intentionally synchronous so the
//! audit-write path stays non-async — the inference fast path must
//! never await on a disk flush.
//!
//! Retention / rotation is a thin layer: the store exposes
//! [`AuditStore::rotate_if_due`], which checks whether the current
//! day boundary has been crossed and resets the in-memory tail (the
//! WAL keeps history). Pruning beyond retention is the operator's
//! responsibility; the default config tracks 7 years (HIPAA
//! requirement) so they have time to put a pruning job in place.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::entry::{AuditEntry, CorrelationFields};

/// Default retention horizon (7 years, HIPAA minimum).
pub const DEFAULT_RETENTION_DAYS: u32 = 7 * 365;

/// Configuration for the audit store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditStoreConfig {
    /// Maximum entries to retain in the in-memory tail. Once
    /// exceeded, the oldest entries are evicted from memory (they
    /// remain in the WAL if one is configured).
    #[serde(default = "AuditStoreConfig::default_max_in_memory")]
    pub max_in_memory: usize,
    /// Optional WAL path. When set, every appended entry is
    /// serialised as one JSON-lines record.
    #[serde(default)]
    pub wal_path: Option<PathBuf>,
    /// Retention horizon in days. Documented for operators; the
    /// store itself does not prune beyond [`Self::max_in_memory`].
    #[serde(default = "AuditStoreConfig::default_retention_days")]
    pub retention_days: u32,
    /// Cap on the number of offline correlation events queued
    /// before the oldest are dropped. Drop count is surfaced on
    /// [`AuditStore::drop_counters`].
    #[serde(default = "AuditStoreConfig::default_offline_queue_capacity")]
    pub offline_queue_capacity: usize,
}

impl Default for AuditStoreConfig {
    fn default() -> Self {
        Self {
            max_in_memory: Self::default_max_in_memory(),
            wal_path: None,
            retention_days: Self::default_retention_days(),
            offline_queue_capacity: Self::default_offline_queue_capacity(),
        }
    }
}

impl AuditStoreConfig {
    /// Default in-memory tail (8192 entries).
    pub fn default_max_in_memory() -> usize {
        8192
    }

    /// Default retention horizon ([`DEFAULT_RETENTION_DAYS`]).
    pub fn default_retention_days() -> u32 {
        DEFAULT_RETENTION_DAYS
    }

    /// Default offline-queue capacity (4096 events).
    pub fn default_offline_queue_capacity() -> usize {
        4096
    }
}

/// Pluggable sealing hook. The default [`NullSealer`] passes bytes
/// through unchanged; production wires this to a vault-backed AEAD
/// so the on-disk WAL is encrypted at rest.
pub trait StoreSealer: Send + Sync + std::fmt::Debug {
    /// Transform plaintext bytes into the form that will be written
    /// to disk.
    fn seal(&self, plaintext: &[u8]) -> Vec<u8>;

    /// Recover plaintext from a record produced by [`Self::seal`]. The audit
    /// reader uses this to verify the persisted WAL from its true head (U2).
    /// Errors when a record cannot be authenticated (wrong key or corruption) —
    /// fail closed.
    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError>;
}

/// No-op sealer. Used for bring-up and tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSealer;

impl StoreSealer for NullSealer {
    fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        plaintext.to_vec()
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError> {
        Ok(sealed.to_vec())
    }
}

/// Store-level error.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Failed to open or write to the WAL.
    #[error("WAL I/O error at {path}: {source}")]
    WalIo {
        /// Path being written.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A persisted WAL record failed to unseal (wrong key or corrupt AEAD).
    #[error("WAL record failed to unseal (wrong key or corrupt)")]
    WalUnseal,
    /// A persisted WAL record could not be decoded (bad hex framing or JSON).
    #[error("WAL record is corrupt: {detail}")]
    WalCorrupt {
        /// What failed to decode.
        detail: String,
    },
}

/// Counts of dropped or evicted items, surfaced for dashboards.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreDropCounters {
    /// Entries evicted from the in-memory tail (still in WAL).
    pub in_memory_evicted: u64,
    /// Offline correlation events dropped because the queue was
    /// full.
    pub offline_events_dropped: u64,
}

#[derive(Debug)]
struct Inner {
    config: AuditStoreConfig,
    entries: VecDeque<AuditEntry>,
    offline_queue: VecDeque<CorrelationFields>,
    online: bool,
    drops: StoreDropCounters,
    last_rotation_day: i64,
}

/// In-memory + optional WAL audit store.
#[derive(Clone)]
pub struct AuditStore {
    inner: Arc<Mutex<Inner>>,
    sealer: Arc<dyn StoreSealer>,
}

impl std::fmt::Debug for AuditStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditStore").finish_non_exhaustive()
    }
}

impl Default for AuditStore {
    fn default() -> Self {
        Self::new(AuditStoreConfig::default(), Arc::new(NullSealer))
    }
}

impl AuditStore {
    /// Build a store with the given config and sealer.
    pub fn new(config: AuditStoreConfig, sealer: Arc<dyn StoreSealer>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                entries: VecDeque::new(),
                offline_queue: VecDeque::new(),
                online: true,
                drops: StoreDropCounters::default(),
                last_rotation_day: current_unix_day(),
            })),
            sealer,
        }
    }

    /// Active configuration (cloned).
    pub fn config(&self) -> AuditStoreConfig {
        self.inner
            .lock()
            .expect("audit store poisoned")
            .config
            .clone()
    }

    /// Append a finalised audit entry. Returns the entry's id for
    /// caller correlation.
    pub fn append(&self, entry: AuditEntry) -> Result<u64, StoreError> {
        let id = entry.id;
        let wal_line = serde_json::to_vec(&entry).expect("serialise audit entry");
        // Mutate in-memory tail under the lock.
        {
            let mut guard = self.inner.lock().expect("audit store poisoned");
            guard.entries.push_back(entry);
            if guard.entries.len() > guard.config.max_in_memory {
                let to_drop = guard.entries.len() - guard.config.max_in_memory;
                for _ in 0..to_drop {
                    guard.entries.pop_front();
                    guard.drops.in_memory_evicted = guard.drops.in_memory_evicted.saturating_add(1);
                }
            }
        }
        // WAL write happens outside the in-memory lock so a slow
        // disk doesn't block readers.
        let wal_path = self.config().wal_path;
        if let Some(path) = wal_path {
            let sealed = self.sealer.seal(&wal_line);
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|source| StoreError::WalIo {
                    path: path.display().to_string(),
                    source,
                })?;
            // Frame each record as hex + '\n'. Sealed (AEAD) records are binary
            // and may contain 0x0A, so raw bytes cannot be newline-delimited; hex
            // keeps exactly one record per line and round-trips in `read_wal`.
            let mut w = BufWriter::new(&mut f);
            let line = hex::encode(&sealed);
            w.write_all(line.as_bytes())
                .and_then(|()| w.write_all(b"\n"))
                .map_err(|source| StoreError::WalIo {
                    path: path.display().to_string(),
                    source,
                })?;
            w.flush().map_err(|source| StoreError::WalIo {
                path: path.display().to_string(),
                source,
            })?;
        }
        Ok(id)
    }

    /// Snapshot of the in-memory tail. Cloned so callers don't hold
    /// the lock.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.inner
            .lock()
            .expect("audit store poisoned")
            .entries
            .iter()
            .cloned()
            .collect()
    }

    /// `true` when a WAL path is configured (durable history exists on disk).
    pub fn has_wal(&self) -> bool {
        self.inner
            .lock()
            .expect("audit store poisoned")
            .config
            .wal_path
            .is_some()
    }

    /// Read and unseal the entire persisted WAL in append (id) order. `Ok(None)`
    /// when no WAL is configured; `Ok(Some(_))` (possibly empty) otherwise. Every
    /// framing / unseal / decode failure is an error — fail-closed for the U2
    /// verifier. O(total entries): the operator verify path, not the append hot
    /// path.
    pub fn read_wal(&self) -> Result<Option<Vec<AuditEntry>>, StoreError> {
        let Some(path) = self.config().wal_path else {
            return Ok(None);
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
            Err(source) => {
                return Err(StoreError::WalIo {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let mut out = Vec::new();
        for line in raw.lines() {
            if line.is_empty() {
                continue;
            }
            let sealed = hex::decode(line).map_err(|e| StoreError::WalCorrupt {
                detail: format!("hex: {e}"),
            })?;
            let plaintext = self.sealer.unseal(&sealed)?;
            let entry: AuditEntry =
                serde_json::from_slice(&plaintext).map_err(|e| StoreError::WalCorrupt {
                    detail: format!("json: {e}"),
                })?;
            out.push(entry);
        }
        Ok(Some(out))
    }

    /// Tail length.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("audit store poisoned")
            .entries
            .len()
    }

    /// `true` when no entries are buffered in memory (WAL may still
    /// have history).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop counters (in-memory evictions, dropped offline events).
    pub fn drop_counters(&self) -> StoreDropCounters {
        self.inner.lock().expect("audit store poisoned").drops
    }

    /// Mark the store online (cloud trust core reachable).
    pub fn mark_online(&self) {
        self.inner.lock().expect("audit store poisoned").online = true;
    }

    /// Mark the store offline. Subsequent calls to
    /// [`Self::enqueue_correlation`] buffer events for later
    /// replay.
    pub fn mark_offline(&self) {
        self.inner.lock().expect("audit store poisoned").online = false;
    }

    /// `true` when the store believes the cloud audit destination
    /// is reachable.
    pub fn is_online(&self) -> bool {
        self.inner.lock().expect("audit store poisoned").online
    }

    /// Enqueue a correlation event for cloud sync. When online, the
    /// event is still queued — flushing is the caller's job (it
    /// owns the sync transport).
    pub fn enqueue_correlation(&self, event: CorrelationFields) {
        let mut guard = self.inner.lock().expect("audit store poisoned");
        if guard.offline_queue.len() >= guard.config.offline_queue_capacity {
            guard.offline_queue.pop_front();
            guard.drops.offline_events_dropped =
                guard.drops.offline_events_dropped.saturating_add(1);
        }
        guard.offline_queue.push_back(event);
    }

    /// Pending correlation events in the offline queue.
    pub fn offline_queue_len(&self) -> usize {
        self.inner
            .lock()
            .expect("audit store poisoned")
            .offline_queue
            .len()
    }

    /// Drain the offline queue, returning every queued correlation
    /// event in arrival order. Called by the cloud-sync worker once
    /// connectivity returns.
    pub fn drain_offline_queue(&self) -> Vec<CorrelationFields> {
        let mut guard = self.inner.lock().expect("audit store poisoned");
        guard.offline_queue.drain(..).collect()
    }

    /// Check whether the current wall-clock day differs from the
    /// last recorded rotation day. When it does, returns `true` and
    /// records the new day; the caller is expected to flush /
    /// archive the in-memory tail if it wants daily rotation
    /// semantics. The store itself does not delete data on rotation.
    pub fn rotate_if_due(&self) -> bool {
        let today = current_unix_day();
        let mut guard = self.inner.lock().expect("audit store poisoned");
        if today == guard.last_rotation_day {
            false
        } else {
            guard.last_rotation_day = today;
            true
        }
    }
}

fn current_unix_day() -> i64 {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    // Saturating cast keeps the day bucket monotonic even on the
    // far side of i64::MAX seconds (year ~292 billion); the value
    // is only compared to itself, never absolute.
    i64::try_from(now_secs).unwrap_or(i64::MAX) / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::entry::{CHAIN_HASH_LEN, RoutingDecision, masked_request_hash};
    use crate::policy::composer::ModuleId;

    fn entry(id: u64) -> AuditEntry {
        AuditEntry {
            id,
            timestamp_unix_nanos: 0,
            request_hash: masked_request_hash(format!("body-{id}").as_bytes()),
            decision: RoutingDecision::LocalOnly,
            modules_applied: vec![ModuleId::Hipaa],
            rules_fired: vec![],
            flags: vec![],
            routing_reason: format!("rule.{id}"),
            user_profile: "hmac:0".into(),
            correlation: CorrelationFields {
                credential_event_id: None,
                lamprey_decision_id: format!("dec-{id}"),
                mai_request_id: format!("req-{id}"),
                tenant: "t".into(),
                subject_hash: "hmac:0".into(),
                service_identity: None,
                policy_version: "v1".into(),
                trust_bundle_version: "v1".into(),
                decision: RoutingDecision::LocalOnly,
            },
            previous_hash: [0u8; CHAIN_HASH_LEN],
            signature: None,
        }
    }

    #[test]
    fn append_and_snapshot() {
        let store = AuditStore::default();
        for i in 0..3 {
            store.append(entry(i)).unwrap();
        }
        assert_eq!(store.len(), 3);
        let snap = store.entries();
        assert_eq!(snap.iter().map(|e| e.id).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn eviction_increments_drop_counter() {
        let store = AuditStore::new(
            AuditStoreConfig {
                max_in_memory: 2,
                ..AuditStoreConfig::default()
            },
            Arc::new(NullSealer),
        );
        for i in 0..5 {
            store.append(entry(i)).unwrap();
        }
        assert_eq!(store.len(), 2);
        assert_eq!(store.drop_counters().in_memory_evicted, 3);
    }

    #[test]
    fn wal_roundtrip_in_temp_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mai-audit-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = AuditStore::new(
            AuditStoreConfig {
                wal_path: Some(path.clone()),
                ..AuditStoreConfig::default()
            },
            Arc::new(NullSealer),
        );
        for i in 0..3 {
            store.append(entry(i)).unwrap();
        }
        // On-disk framing is hex-per-line now (safe for binary/AEAD records).
        let raw = std::fs::read_to_string(&path).expect("read wal");
        assert_eq!(raw.lines().count(), 3);
        assert!(
            raw.lines()
                .all(|l| l.bytes().all(|b| b.is_ascii_hexdigit())),
            "each WAL line must be hex"
        );
        // Replay through the reader: each record round-trips to its AuditEntry.
        let replayed = store.read_wal().expect("read_wal").expect("wal configured");
        assert_eq!(
            replayed.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn offline_queue_buffers_and_drains() {
        let store = AuditStore::default();
        store.mark_offline();
        assert!(!store.is_online());
        for i in 0..3 {
            store.enqueue_correlation(entry(i).correlation);
        }
        assert_eq!(store.offline_queue_len(), 3);
        store.mark_online();
        let drained = store.drain_offline_queue();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].lamprey_decision_id, "dec-0");
        assert_eq!(store.offline_queue_len(), 0);
    }

    #[test]
    fn offline_queue_caps_at_capacity() {
        let store = AuditStore::new(
            AuditStoreConfig {
                offline_queue_capacity: 2,
                ..AuditStoreConfig::default()
            },
            Arc::new(NullSealer),
        );
        for i in 0..5 {
            store.enqueue_correlation(entry(i).correlation);
        }
        assert_eq!(store.offline_queue_len(), 2);
        assert_eq!(store.drop_counters().offline_events_dropped, 3);
    }

    #[test]
    fn rotate_if_due_only_fires_once_per_day() {
        let store = AuditStore::default();
        // First call after construction returns false (same day).
        assert!(!store.rotate_if_due());
        // Force a day change by reaching in.
        {
            let mut g = store.inner.lock().unwrap();
            g.last_rotation_day -= 1;
        }
        assert!(store.rotate_if_due());
        assert!(!store.rotate_if_due());
    }

    #[test]
    fn default_retention_is_seven_years() {
        assert_eq!(AuditStoreConfig::default().retention_days, 7 * 365);
    }
}
