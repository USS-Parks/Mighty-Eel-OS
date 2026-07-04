//! Soft eviction: KV cache offload to CPU pinned memory.
//!
//! Standard eviction destroys a sequence's KV cache; restoring it requires a
//! full re-prefill (seconds). Soft eviction instead copies the KV tensors
//! from GPU VRAM to CPU pinned memory, releasing GPU memory while preserving
//! the sequence state. A restore copies the bytes back. The actual byte
//! movement is the inference engine's responsibility; this module tracks the
//! state machine and budgets that decision rests on.
//!
//! State transitions:
//!
//! ```text
//!     Active --offload--> Offloading --done--> Offloaded
//!     Offloaded --restore--> Restoring --done--> Active
//! ```
//!
//! Intermediate states (Offloading / Restoring) let callers distinguish a
//! sequence that is committed to a transition from one that is idle.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::SequenceId;

/// Lifecycle state of a sequence with respect to soft eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoftEvictionState {
    /// KV cache resides in GPU VRAM.
    Active,
    /// Transition from GPU to CPU is in flight.
    Offloading,
    /// KV cache resides in CPU pinned memory.
    Offloaded,
    /// Transition from CPU back to GPU is in flight.
    Restoring,
}

/// Soft-eviction errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OffloadError {
    /// CPU pinned-memory budget would be exceeded.
    #[error("cpu offload budget exhausted: would use {would_use} of {budget} bytes")]
    BudgetExceeded {
        /// What the new allocation would push usage to.
        would_use: u64,
        /// Configured CPU budget.
        budget: u64,
    },
    /// Operation is invalid for the current state.
    #[error("sequence {seq_id} cannot transition from {current:?}")]
    InvalidState {
        /// Sequence under consideration.
        seq_id: SequenceId,
        /// Observed state.
        current: SoftEvictionState,
    },
    /// Sequence is not known to the offload manager.
    #[error("sequence {0} is not tracked")]
    UnknownSequence(SequenceId),
}

/// Configuration for the offload manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffloadConfig {
    /// CPU pinned-memory budget for offloaded KV tensors.
    pub cpu_budget_bytes: u64,
    /// Expected latency to copy GPU → CPU.
    #[serde(default = "default_offload_ms")]
    pub offload_latency_ms: u64,
    /// Expected latency to copy CPU → GPU.
    #[serde(default = "default_restore_ms")]
    pub restore_latency_ms: u64,
}

fn default_offload_ms() -> u64 {
    50
}

fn default_restore_ms() -> u64 {
    100
}

impl Default for OffloadConfig {
    fn default() -> Self {
        Self {
            cpu_budget_bytes: 16 * 1_000_000_000, // 16 GB
            offload_latency_ms: default_offload_ms(),
            restore_latency_ms: default_restore_ms(),
        }
    }
}

/// Bookkeeping for a single offloaded sequence.
#[derive(Debug, Clone)]
pub struct OffloadRecord {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Current lifecycle state.
    pub state: SoftEvictionState,
    /// Bytes occupied in CPU pinned memory while offloaded.
    pub cpu_bytes: u64,
    /// When the most recent transition began.
    pub state_changed_at: Instant,
}

/// Tracks soft-eviction state and CPU pinned-memory budget.
#[derive(Debug)]
pub struct OffloadManager {
    config: OffloadConfig,
    records: Mutex<HashMap<SequenceId, OffloadRecord>>,
    cpu_used_bytes: Mutex<u64>,
}

impl OffloadManager {
    /// Build a new manager.
    pub fn new(config: OffloadConfig) -> Self {
        Self {
            config,
            records: Mutex::new(HashMap::new()),
            cpu_used_bytes: Mutex::new(0),
        }
    }

    /// Begin an offload: reserve CPU bytes and mark the sequence Offloading.
    pub fn begin_offload(&self, seq_id: SequenceId, cpu_bytes: u64) -> Result<(), OffloadError> {
        let mut used = self.cpu_used_bytes.lock().unwrap();
        let would_use = used.saturating_add(cpu_bytes);
        if would_use > self.config.cpu_budget_bytes {
            return Err(OffloadError::BudgetExceeded {
                would_use,
                budget: self.config.cpu_budget_bytes,
            });
        }
        let mut records = self.records.lock().unwrap();
        if let Some(existing) = records.get(&seq_id) {
            return Err(OffloadError::InvalidState {
                seq_id,
                current: existing.state,
            });
        }
        records.insert(
            seq_id,
            OffloadRecord {
                seq_id,
                state: SoftEvictionState::Offloading,
                cpu_bytes,
                state_changed_at: Instant::now(),
            },
        );
        *used = would_use;
        Ok(())
    }

    /// Finish an offload — caller has finished copying bytes to CPU.
    pub fn complete_offload(&self, seq_id: SequenceId) -> Result<(), OffloadError> {
        self.transition(
            seq_id,
            SoftEvictionState::Offloading,
            SoftEvictionState::Offloaded,
        )
    }

    /// Begin restoring an offloaded sequence back to GPU.
    pub fn begin_restore(&self, seq_id: SequenceId) -> Result<(), OffloadError> {
        self.transition(
            seq_id,
            SoftEvictionState::Offloaded,
            SoftEvictionState::Restoring,
        )
    }

    /// Finish a restore — caller has finished copying bytes back to GPU.
    /// The CPU bytes are released to the pinned-memory pool.
    pub fn complete_restore(&self, seq_id: SequenceId) -> Result<(), OffloadError> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&seq_id)
            .ok_or(OffloadError::UnknownSequence(seq_id))?;
        if record.state != SoftEvictionState::Restoring {
            return Err(OffloadError::InvalidState {
                seq_id,
                current: record.state,
            });
        }
        let freed = record.cpu_bytes;
        records.remove(&seq_id);
        drop(records);
        let mut used = self.cpu_used_bytes.lock().unwrap();
        *used = used.saturating_sub(freed);
        Ok(())
    }

    /// Drop the offload record and release CPU bytes — used when a sequence
    /// is destroyed entirely without restoring.
    pub fn discard(&self, seq_id: SequenceId) -> Result<u64, OffloadError> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .remove(&seq_id)
            .ok_or(OffloadError::UnknownSequence(seq_id))?;
        drop(records);
        let mut used = self.cpu_used_bytes.lock().unwrap();
        *used = used.saturating_sub(record.cpu_bytes);
        Ok(record.cpu_bytes)
    }

    /// Lookup the current state of a sequence, if tracked.
    pub fn state(&self, seq_id: SequenceId) -> Option<SoftEvictionState> {
        self.records.lock().unwrap().get(&seq_id).map(|r| r.state)
    }

    /// Bytes currently held in the CPU pinned-memory pool.
    pub fn cpu_used_bytes(&self) -> u64 {
        *self.cpu_used_bytes.lock().unwrap()
    }

    /// Configured CPU budget.
    pub fn cpu_budget_bytes(&self) -> u64 {
        self.config.cpu_budget_bytes
    }

    /// Snapshot of all tracked records.
    pub fn snapshot(&self) -> Vec<OffloadRecord> {
        self.records.lock().unwrap().values().cloned().collect()
    }

    fn transition(
        &self,
        seq_id: SequenceId,
        expected: SoftEvictionState,
        next: SoftEvictionState,
    ) -> Result<(), OffloadError> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&seq_id)
            .ok_or(OffloadError::UnknownSequence(seq_id))?;
        if record.state != expected {
            return Err(OffloadError::InvalidState {
                seq_id,
                current: record.state,
            });
        }
        record.state = next;
        record.state_changed_at = Instant::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(budget: u64) -> OffloadManager {
        OffloadManager::new(OffloadConfig {
            cpu_budget_bytes: budget,
            ..OffloadConfig::default()
        })
    }

    #[test]
    fn test_offload_restore_round_trip() {
        let m = manager(1_000_000);
        let seq = SequenceId::new();
        m.begin_offload(seq, 100).unwrap();
        assert_eq!(m.state(seq), Some(SoftEvictionState::Offloading));
        m.complete_offload(seq).unwrap();
        assert_eq!(m.state(seq), Some(SoftEvictionState::Offloaded));
        assert_eq!(m.cpu_used_bytes(), 100);

        m.begin_restore(seq).unwrap();
        assert_eq!(m.state(seq), Some(SoftEvictionState::Restoring));
        m.complete_restore(seq).unwrap();
        assert_eq!(m.state(seq), None);
        assert_eq!(m.cpu_used_bytes(), 0);
    }

    #[test]
    fn test_offload_budget_exceeded_rejects() {
        let m = manager(100);
        let result = m.begin_offload(SequenceId::new(), 101);
        assert!(matches!(result, Err(OffloadError::BudgetExceeded { .. })));
        assert_eq!(m.cpu_used_bytes(), 0);
    }

    #[test]
    fn test_invalid_state_transitions_are_rejected() {
        let m = manager(1_000);
        let seq = SequenceId::new();
        // restore before offload is invalid
        assert!(matches!(
            m.begin_restore(seq),
            Err(OffloadError::UnknownSequence(_))
        ));
        m.begin_offload(seq, 100).unwrap();
        // double offload (without completing) is invalid
        assert!(matches!(
            m.begin_offload(seq, 100),
            Err(OffloadError::InvalidState { .. })
        ));
        // restore before complete_offload is invalid
        assert!(matches!(
            m.begin_restore(seq),
            Err(OffloadError::InvalidState { .. })
        ));
    }

    #[test]
    fn test_discard_releases_budget_without_restore() {
        let m = manager(1_000);
        let seq = SequenceId::new();
        m.begin_offload(seq, 200).unwrap();
        m.complete_offload(seq).unwrap();
        let freed = m.discard(seq).unwrap();
        assert_eq!(freed, 200);
        assert_eq!(m.cpu_used_bytes(), 0);
        assert_eq!(m.state(seq), None);
    }
}
