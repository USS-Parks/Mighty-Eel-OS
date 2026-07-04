//! Priority-based preemption with starvation prevention.
//!
//! When an instance is at capacity and a higher-priority request arrives, the
//! scheduler may *preempt* a lower-priority sequence — pausing it (via soft
//! eviction to warm tier) so the new request can take its slot. Preempted
//! sequences resume when capacity frees, with a one-step priority boost to
//! prevent starvation under sustained pressure.
//!
//! Hierarchy (lower numeric value = higher priority):
//!
//!   System (0) > High (1) > Normal (2) > Background (3)
//!
//! Rules:
//! - System can preempt anything below it.
//! - High can preempt Normal and Background.
//! - Normal cannot preempt anything.
//! - Background is always preemptable.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use thiserror::Error;

use crate::types::{Priority, SequenceId};

/// Errors raised by the preemption manager.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreemptionError {
    /// The incoming request's priority does not outrank the candidate.
    #[error("priority {incoming:?} cannot preempt {existing:?}")]
    NotPreemptable {
        /// Priority of the new request asking for capacity.
        incoming: Priority,
        /// Priority of the currently-running sequence.
        existing: Priority,
    },
    /// Sequence is not currently preempted (resume called incorrectly).
    #[error("sequence {0} is not in the preempted set")]
    NotPreempted(SequenceId),
}

/// Bookkeeping for a single preempted sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreemptedSequence {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Priority the sequence had before preemption.
    pub original_priority: Priority,
    /// When the preemption began.
    pub preempted_at: Instant,
}

/// Stateless priority comparison: does `incoming` outrank `existing`?
pub fn can_preempt(incoming: Priority, existing: Priority) -> bool {
    // Priority is `Ord` such that lower discriminant == higher priority.
    incoming < existing
}

/// Stateful tracker for the in-flight preempted set.
#[derive(Debug, Default)]
pub struct PreemptionManager {
    preempted: Mutex<HashMap<SequenceId, PreemptedSequence>>,
}

impl PreemptionManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to preempt `existing` in favor of an `incoming` priority. On
    /// success, records the preempted sequence so it can be resumed later.
    pub fn preempt(
        &self,
        seq_id: SequenceId,
        existing: Priority,
        incoming: Priority,
    ) -> Result<PreemptedSequence, PreemptionError> {
        if !can_preempt(incoming, existing) {
            return Err(PreemptionError::NotPreemptable { incoming, existing });
        }
        let record = PreemptedSequence {
            seq_id,
            original_priority: existing,
            preempted_at: Instant::now(),
        };
        self.preempted
            .lock()
            .unwrap()
            .insert(seq_id, record.clone());
        Ok(record)
    }

    /// Resume a preempted sequence and return its boosted priority.
    ///
    /// Starvation prevention: a preempted sequence resumes one step higher
    /// than its original priority (Background → Normal, Normal → High,
    /// High → High). System priority remains unchanged.
    pub fn resume(&self, seq_id: SequenceId) -> Result<Priority, PreemptionError> {
        let mut preempted = self.preempted.lock().unwrap();
        let record = preempted
            .remove(&seq_id)
            .ok_or(PreemptionError::NotPreempted(seq_id))?;
        Ok(boost_priority(record.original_priority))
    }

    /// Inspect the current set of preempted sequences.
    pub fn snapshot(&self) -> Vec<PreemptedSequence> {
        self.preempted.lock().unwrap().values().cloned().collect()
    }

    /// True if the sequence is currently preempted.
    pub fn is_preempted(&self, seq_id: SequenceId) -> bool {
        self.preempted.lock().unwrap().contains_key(&seq_id)
    }
}

/// One-step priority boost. Saturating at `High`.
pub fn boost_priority(original: Priority) -> Priority {
    match original {
        Priority::System => Priority::System,
        Priority::High => Priority::High,
        Priority::Normal => Priority::High,
        Priority::Background => Priority::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_preempt_hierarchy() {
        assert!(can_preempt(Priority::System, Priority::High));
        assert!(can_preempt(Priority::System, Priority::Background));
        assert!(can_preempt(Priority::High, Priority::Normal));
        assert!(can_preempt(Priority::High, Priority::Background));
        // Normal cannot preempt anything except Background.
        assert!(!can_preempt(Priority::Normal, Priority::Normal));
        assert!(!can_preempt(Priority::Normal, Priority::High));
        // Equal priority cannot preempt.
        assert!(!can_preempt(Priority::High, Priority::High));
    }

    #[test]
    fn test_normal_cannot_preempt_normal() {
        let m = PreemptionManager::new();
        let result = m.preempt(SequenceId::new(), Priority::Normal, Priority::Normal);
        assert!(matches!(
            result,
            Err(PreemptionError::NotPreemptable { .. })
        ));
    }

    #[test]
    fn test_high_preempts_background_and_resume_boosts() {
        let m = PreemptionManager::new();
        let seq = SequenceId::new();
        m.preempt(seq, Priority::Background, Priority::High)
            .unwrap();
        assert!(m.is_preempted(seq));
        let resumed = m.resume(seq).unwrap();
        // Background → Normal on resume (starvation prevention).
        assert_eq!(resumed, Priority::Normal);
        assert!(!m.is_preempted(seq));
    }

    #[test]
    fn test_system_can_preempt_anything() {
        let m = PreemptionManager::new();
        for victim in [Priority::High, Priority::Normal, Priority::Background] {
            let seq = SequenceId::new();
            m.preempt(seq, victim, Priority::System).unwrap();
            assert!(m.is_preempted(seq));
            m.resume(seq).unwrap();
        }
    }

    #[test]
    fn test_resume_unknown_sequence_errors() {
        let m = PreemptionManager::new();
        let result = m.resume(SequenceId::new());
        assert!(matches!(result, Err(PreemptionError::NotPreempted(_))));
    }

    #[test]
    fn test_boost_priority_table() {
        assert_eq!(boost_priority(Priority::System), Priority::System);
        assert_eq!(boost_priority(Priority::High), Priority::High);
        assert_eq!(boost_priority(Priority::Normal), Priority::High);
        assert_eq!(boost_priority(Priority::Background), Priority::Normal);
    }

    #[test]
    fn test_starvation_prevention_two_cycles() {
        // A Background sequence preempted, resumed (becomes Normal), then
        // preempted again — on the second resume it should keep Normal+ status
        // (boosted to High because Normal → High).
        let m = PreemptionManager::new();
        let seq = SequenceId::new();
        m.preempt(seq, Priority::Background, Priority::High)
            .unwrap();
        let first = m.resume(seq).unwrap();
        assert_eq!(first, Priority::Normal);

        m.preempt(seq, first, Priority::System).unwrap();
        let second = m.resume(seq).unwrap();
        assert_eq!(second, Priority::High);
    }
}
