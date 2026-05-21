//! Emergency-only preemption policy for active batch members.
//!
//! Default policy: NEVER preempt active batch members unless emergency
//! eviction is required (VRAM > 95%). When preemption is unavoidable:
//!
//! 1. Prefer the sequence closest to completion (highest progress)
//! 2. Prefer lower-priority sequences
//! 3. Log the event as critical (this should be rare)
//!
//! Preemption is the absolute last resort. The admission control and
//! proactive eviction systems should prevent reaching this point.

use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::types::{Priority, SequenceId};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Preemption policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreemptionConfig {
    /// VRAM usage fraction above which preemption is permitted (default 0.95).
    #[serde(default = "default_emergency_threshold")]
    pub emergency_threshold: f64,

    /// Weight for completion progress in preemption scoring.
    /// Higher = more likely to preempt sequences close to finishing.
    #[serde(default = "default_progress_weight")]
    pub progress_weight: f64,

    /// Weight for priority in preemption scoring.
    /// Higher = more likely to preempt lower-priority sequences.
    #[serde(default = "default_priority_weight")]
    pub priority_weight: f64,
}

fn default_emergency_threshold() -> f64 {
    0.95
}
fn default_progress_weight() -> f64 {
    0.4
}
fn default_priority_weight() -> f64 {
    0.6
}

impl Default for PreemptionConfig {
    fn default() -> Self {
        Self {
            emergency_threshold: default_emergency_threshold(),
            progress_weight: default_progress_weight(),
            priority_weight: default_priority_weight(),
        }
    }
}

// ---------------------------------------------------------------------------
// Preemption candidate
// ---------------------------------------------------------------------------

/// Information about an active batch member needed for preemption scoring.
#[derive(Debug, Clone)]
pub struct PreemptionCandidate {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Request priority.
    pub priority: Priority,
    /// Estimated completion progress (0.0 = just started, 1.0 = nearly done).
    /// Computed as: generated_tokens / max_tokens.
    pub completion_progress: f64,
    /// KV cache bytes consumed by this sequence.
    pub kv_bytes: u64,
}

/// Result of a preemption decision.
#[derive(Debug, Clone)]
pub struct PreemptionResult {
    /// Sequences selected for preemption, ordered by preemption priority.
    pub victims: Vec<SequenceId>,
    /// Total bytes that will be freed by preempting these sequences.
    pub bytes_freed: u64,
    /// Whether this was triggered by emergency conditions.
    pub is_emergency: bool,
}

// ---------------------------------------------------------------------------
// Preemption policy
// ---------------------------------------------------------------------------

/// The preemption policy. Evaluates active batch members and selects
/// victims when emergency eviction is required.
#[derive(Debug, Clone)]
pub struct PreemptionPolicy {
    config: PreemptionConfig,
}

impl PreemptionPolicy {
    /// Create a new preemption policy with the given configuration.
    pub fn new(config: PreemptionConfig) -> Self {
        Self { config }
    }

    /// Update configuration at runtime.
    pub fn update_config(&mut self, config: PreemptionConfig) {
        self.config = config;
    }

    /// Current configuration.
    pub fn config(&self) -> &PreemptionConfig {
        &self.config
    }

    /// Check whether preemption is permitted given current VRAM pressure.
    pub fn is_emergency(&self, vram_usage_fraction: f64) -> bool {
        vram_usage_fraction >= self.config.emergency_threshold
    }

    /// Select victims for preemption from the active batch.
    ///
    /// Returns an empty result if preemption is not an emergency.
    /// When `needed_bytes` > 0 and we're in emergency, selects the
    /// minimum number of victims to free at least `needed_bytes`.
    ///
    /// System priority sequences are NEVER preempted.
    pub fn select_victims(
        &self,
        vram_usage_fraction: f64,
        candidates: &[PreemptionCandidate],
        needed_bytes: u64,
    ) -> PreemptionResult {
        // Not an emergency: no preemption permitted
        if !self.is_emergency(vram_usage_fraction) {
            return PreemptionResult {
                victims: Vec::new(),
                bytes_freed: 0,
                is_emergency: false,
            };
        }

        // Score all candidates (higher score = better preemption target)
        let mut scored: Vec<(usize, f64)> = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.priority != Priority::System) // Never preempt System
            .map(|(i, c)| (i, self.preemption_score(c)))
            .collect();

        // Sort descending by score (best victim first)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Select minimum victims to free needed_bytes
        let mut victims = Vec::new();
        let mut freed = 0_u64;

        for (idx, score) in &scored {
            if freed >= needed_bytes {
                break;
            }
            let candidate = &candidates[*idx];
            victims.push(candidate.seq_id);
            freed += candidate.kv_bytes;

            error!(
                seq = %candidate.seq_id,
                priority = %candidate.priority,
                progress = candidate.completion_progress,
                score = score,
                freed_mb = candidate.kv_bytes / 1_000_000,
                vram_frac = vram_usage_fraction,
                "CRITICAL: Preempting active batch member (emergency eviction)"
            );
        }

        if !victims.is_empty() {
            warn!(
                count = victims.len(),
                freed_mb = freed / 1_000_000,
                needed_mb = needed_bytes / 1_000_000,
                "Emergency preemption completed"
            );
        }

        PreemptionResult {
            victims,
            bytes_freed: freed,
            is_emergency: true,
        }
    }

    /// Compute preemption score for a candidate. Higher = better preemption target.
    ///
    /// Score = progress_weight * completion_progress + priority_weight * priority_score
    ///
    /// - completion_progress: closer to done = higher score (less wasted work if preempted)
    /// - priority_score: Background=1.0, Normal=0.66, High=0.33, System=never
    fn preemption_score(&self, candidate: &PreemptionCandidate) -> f64 {
        let priority_score = match candidate.priority {
            Priority::System => 0.0, // should never reach here (filtered out)
            Priority::High => 0.33,
            Priority::Normal => 0.66,
            Priority::Background => 1.0,
        };

        self.config.progress_weight * candidate.completion_progress
            + self.config.priority_weight * priority_score
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> PreemptionPolicy {
        PreemptionPolicy::new(PreemptionConfig::default())
    }

    fn make_candidate(
        priority: Priority,
        progress: f64,
        kv_bytes: u64,
    ) -> PreemptionCandidate {
        PreemptionCandidate {
            seq_id: SequenceId::new(),
            priority,
            completion_progress: progress,
            kv_bytes,
        }
    }

    #[test]
    fn test_no_preemption_below_threshold() {
        let policy = make_policy();
        let candidates = vec![make_candidate(Priority::Background, 0.9, 100_000_000)];
        let result = policy.select_victims(0.90, &candidates, 50_000_000);
        assert!(!result.is_emergency);
        assert!(result.victims.is_empty());
    }

    #[test]
    fn test_preemption_at_threshold() {
        let policy = make_policy();
        let candidates = vec![make_candidate(Priority::Normal, 0.5, 100_000_000)];
        let result = policy.select_victims(0.95, &candidates, 50_000_000);
        assert!(result.is_emergency);
        assert_eq!(result.victims.len(), 1);
        assert_eq!(result.bytes_freed, 100_000_000);
    }

    #[test]
    fn test_system_priority_never_preempted() {
        let policy = make_policy();
        let candidates = vec![
            make_candidate(Priority::System, 0.9, 200_000_000),
            make_candidate(Priority::Normal, 0.5, 100_000_000),
        ];
        let result = policy.select_victims(0.96, &candidates, 50_000_000);
        assert_eq!(result.victims.len(), 1);
        // Only the Normal sequence should be preempted
        assert_eq!(result.victims[0], candidates[1].seq_id);
    }

    #[test]
    fn test_prefers_closer_to_completion() {
        let policy = make_policy();
        let candidates = vec![
            make_candidate(Priority::Normal, 0.1, 100_000_000), // early
            make_candidate(Priority::Normal, 0.9, 100_000_000), // nearly done
        ];
        let result = policy.select_victims(0.96, &candidates, 50_000_000);
        // Should pick the nearly-done one first (higher progress = higher score)
        assert_eq!(result.victims.len(), 1);
        assert_eq!(result.victims[0], candidates[1].seq_id);
    }

    #[test]
    fn test_prefers_lower_priority() {
        let policy = make_policy();
        let candidates = vec![
            make_candidate(Priority::High, 0.5, 100_000_000),
            make_candidate(Priority::Background, 0.5, 100_000_000),
        ];
        let result = policy.select_victims(0.96, &candidates, 50_000_000);
        // Background has higher priority_score (1.0 vs 0.33), should be first victim
        assert_eq!(result.victims.len(), 1);
        assert_eq!(result.victims[0], candidates[1].seq_id);
    }

    #[test]
    fn test_minimum_victims_selected() {
        let policy = make_policy();
        let candidates = vec![
            make_candidate(Priority::Normal, 0.5, 50_000_000),
            make_candidate(Priority::Normal, 0.5, 50_000_000),
            make_candidate(Priority::Normal, 0.5, 50_000_000),
        ];
        // Need 60MB, each victim frees 50MB -> need 2 victims
        let result = policy.select_victims(0.96, &candidates, 60_000_000);
        assert_eq!(result.victims.len(), 2);
        assert!(result.bytes_freed >= 60_000_000);
    }

    #[test]
    fn test_all_system_no_victims() {
        let policy = make_policy();
        let candidates = vec![
            make_candidate(Priority::System, 0.5, 100_000_000),
            make_candidate(Priority::System, 0.9, 200_000_000),
        ];
        let result = policy.select_victims(0.99, &candidates, 50_000_000);
        assert!(result.is_emergency);
        assert!(result.victims.is_empty());
        assert_eq!(result.bytes_freed, 0);
    }

    #[test]
    fn test_empty_candidates() {
        let policy = make_policy();
        let result = policy.select_victims(0.96, &[], 50_000_000);
        assert!(result.is_emergency);
        assert!(result.victims.is_empty());
    }

    #[test]
    fn test_is_emergency_check() {
        let policy = make_policy();
        assert!(!policy.is_emergency(0.94));
        assert!(policy.is_emergency(0.95));
        assert!(policy.is_emergency(0.99));
    }

    #[test]
    fn test_config_update() {
        let mut policy = make_policy();
        assert!((policy.config().emergency_threshold - 0.95).abs() < f64::EPSILON);

        let new_config = PreemptionConfig {
            emergency_threshold: 0.98,
            ..Default::default()
        };
        policy.update_config(new_config);
        assert!((policy.config().emergency_threshold - 0.98).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scoring_formula() {
        let policy = make_policy();
        // Background at 90% progress: 0.4 * 0.9 + 0.6 * 1.0 = 0.36 + 0.60 = 0.96
        let bg = make_candidate(Priority::Background, 0.9, 100_000_000);
        let bg_score = policy.preemption_score(&bg);
        assert!((bg_score - 0.96).abs() < 0.001);

        // High at 10% progress: 0.4 * 0.1 + 0.6 * 0.33 = 0.04 + 0.198 = 0.238
        let high = make_candidate(Priority::High, 0.1, 100_000_000);
        let high_score = policy.preemption_score(&high);
        assert!((high_score - 0.238).abs() < 0.001);

        assert!(bg_score > high_score);
    }
}
