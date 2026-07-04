//! `aog-store` — the deterministic desired-state KV behind Loom's control plane
//! (K2). Keys map to versioned values with a monotonic global revision and a
//! per-key revision; writes carry an optimistic-concurrency precondition
//! (compare-and-set). The apply path is a **pure function of the operation
//! log** — replaying the same `Op` sequence on any [`Backend`] converges to the
//! same state, the property the Raft wrapper (K3) depends on.
//!
//! Engine decision (addendum A4): **redb** — a stable, maintained, pure-Rust
//! embedded store (sled's 1.0 remains perpetually beta). The [`Backend`] trait
//! keeps the deterministic state machine independent of the engine: [`MemBackend`]
//! for tests and Raft's in-core state, [`RedbBackend`] for durability.
//!
//! Intent only. Proof lives in `wsf-ledger`, never here (A1.4).

mod redb_backend;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use redb_backend::RedbBackend;

/// Monotonic revision counter — bumped once per successful mutation.
pub type Revision = u64;

/// A stored value plus its version metadata (etcd/K8s-style).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versioned {
    pub value: Vec<u8>,
    /// Global revision at which this key was first created.
    pub create_revision: Revision,
    /// Global revision at which this key was last modified.
    pub mod_revision: Revision,
    /// Per-key update count (1 on create).
    pub version: u64,
}

/// Optimistic-concurrency precondition on a write (compare-and-set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precondition {
    /// Write unconditionally.
    Any,
    /// The key must not currently exist.
    Absent,
    /// The key's `mod_revision` must equal this value.
    Revision(Revision),
}

/// A desired-state mutation. These are what the Raft log carries (K3); applying
/// a fixed sequence is deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Put {
        key: String,
        value: Vec<u8>,
        expected: Precondition,
    },
    Delete {
        key: String,
        expected: Precondition,
    },
}

/// Outcome of applying one [`Op`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Put { revision: Revision, created: bool },
    Deleted { revision: Revision },
}

/// A store failure — a failed precondition or a backend error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("stale write on {key:?}: expected mod_revision {expected}, found {actual}")]
    StaleRevision {
        key: String,
        expected: Revision,
        actual: Revision,
    },
    #[error("key {key:?} already exists")]
    Exists { key: String },
    #[error("key {key:?} not found")]
    NotFound { key: String },
    #[error("backend: {0}")]
    Backend(String),
}

/// Pluggable persistence for the state machine. Implementations store bytes;
/// all versioning + revision logic lives in [`Store`], keeping apply
/// deterministic regardless of engine.
pub trait Backend {
    /// # Errors
    /// Backend failure.
    fn get(&self, key: &str) -> Result<Option<Versioned>, StoreError>;
    /// # Errors
    /// Backend failure.
    fn insert(&mut self, key: &str, value: &Versioned) -> Result<(), StoreError>;
    /// # Errors
    /// Backend failure.
    fn remove(&mut self, key: &str) -> Result<(), StoreError>;
    /// Entries whose key starts with `prefix`, ascending by key.
    /// # Errors
    /// Backend failure.
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Versioned)>, StoreError>;
    /// Highest `mod_revision` stored (0 when empty) — recovers the global
    /// revision on open.
    /// # Errors
    /// Backend failure.
    fn max_revision(&self) -> Result<Revision, StoreError>;
}

/// In-memory backend: deterministic, the default for tests and Raft's in-core
/// state machine (durability comes from Raft snapshots, not this map).
#[derive(Debug, Default)]
pub struct MemBackend {
    map: BTreeMap<String, Versioned>,
}

impl MemBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Backend for MemBackend {
    fn get(&self, key: &str) -> Result<Option<Versioned>, StoreError> {
        Ok(self.map.get(key).cloned())
    }

    fn insert(&mut self, key: &str, value: &Versioned) -> Result<(), StoreError> {
        self.map.insert(key.to_owned(), value.clone());
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<(), StoreError> {
        self.map.remove(key);
        Ok(())
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Versioned)>, StoreError> {
        Ok(self
            .map
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    fn max_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.map.values().map(|v| v.mod_revision).max().unwrap_or(0))
    }
}

/// The deterministic desired-state store: revision + CAS logic over a [`Backend`].
#[derive(Debug)]
pub struct Store<B: Backend> {
    backend: B,
    revision: Revision,
}

impl<B: Backend> Store<B> {
    /// Open over a backend, recovering the global revision from stored state.
    ///
    /// # Errors
    /// Backend failure while reading the recovered revision.
    pub fn open(backend: B) -> Result<Self, StoreError> {
        let revision = backend.max_revision()?;
        Ok(Self { backend, revision })
    }

    /// The current global revision.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Read one key.
    ///
    /// # Errors
    /// Backend failure.
    pub fn get(&self, key: &str) -> Result<Option<Versioned>, StoreError> {
        self.backend.get(key)
    }

    /// Range by key prefix (ascending).
    ///
    /// # Errors
    /// Backend failure.
    pub fn range(&self, prefix: &str) -> Result<Vec<(String, Versioned)>, StoreError> {
        self.backend.scan_prefix(prefix)
    }

    /// Apply one operation deterministically, enforcing its precondition.
    ///
    /// # Errors
    /// [`StoreError::StaleRevision`] / [`StoreError::Exists`] /
    /// [`StoreError::NotFound`] on a failed precondition; backend failure otherwise.
    pub fn apply(&mut self, op: &Op) -> Result<Applied, StoreError> {
        match op {
            Op::Put {
                key,
                value,
                expected,
            } => self.apply_put(key, value, *expected),
            Op::Delete { key, expected } => self.apply_delete(key, *expected),
        }
    }

    /// Apply a fixed op log in order (op-log replay).
    ///
    /// # Errors
    /// The first failing op's error.
    pub fn apply_all(&mut self, ops: &[Op]) -> Result<Vec<Applied>, StoreError> {
        ops.iter().map(|op| self.apply(op)).collect()
    }

    fn check(
        current: Option<&Versioned>,
        key: &str,
        expected: Precondition,
    ) -> Result<(), StoreError> {
        match expected {
            Precondition::Any => Ok(()),
            Precondition::Absent => {
                if current.is_some() {
                    Err(StoreError::Exists {
                        key: key.to_owned(),
                    })
                } else {
                    Ok(())
                }
            }
            Precondition::Revision(want) => match current {
                Some(v) if v.mod_revision == want => Ok(()),
                Some(v) => Err(StoreError::StaleRevision {
                    key: key.to_owned(),
                    expected: want,
                    actual: v.mod_revision,
                }),
                None => Err(StoreError::NotFound {
                    key: key.to_owned(),
                }),
            },
        }
    }

    fn apply_put(
        &mut self,
        key: &str,
        value: &[u8],
        expected: Precondition,
    ) -> Result<Applied, StoreError> {
        let current = self.backend.get(key)?;
        Self::check(current.as_ref(), key, expected)?;
        let next = self.revision + 1;
        let versioned = match &current {
            Some(prev) => Versioned {
                value: value.to_vec(),
                create_revision: prev.create_revision,
                mod_revision: next,
                version: prev.version + 1,
            },
            None => Versioned {
                value: value.to_vec(),
                create_revision: next,
                mod_revision: next,
                version: 1,
            },
        };
        self.backend.insert(key, &versioned)?;
        self.revision = next;
        Ok(Applied::Put {
            revision: next,
            created: current.is_none(),
        })
    }

    fn apply_delete(&mut self, key: &str, expected: Precondition) -> Result<Applied, StoreError> {
        let current = self.backend.get(key)?;
        Self::check(current.as_ref(), key, expected)?;
        if current.is_none() {
            return Err(StoreError::NotFound {
                key: key.to_owned(),
            });
        }
        let next = self.revision + 1;
        self.backend.remove(key)?;
        self.revision = next;
        Ok(Applied::Deleted { revision: next })
    }
}
