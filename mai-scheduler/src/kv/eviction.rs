//! Multi-factor eviction policy for KV cache management.
//!
//! The eviction scorer computes a score for each cached sequence. Higher
//! scores indicate sequences that should be evicted first. The score is
//! a weighted combination of:
//!
//! - **Idle time**: how long since the sequence was last accessed
//! - **Size**: how much VRAM the sequence consumes
//! - **Priority penalty**: priority-based bias (system priority = never evict)
//! - **Reuse prediction**: estimated likelihood of future reuse
//! - **Batch contribution**: protection bonus for sequences in the active
//!   batch. Prevents normal eviction from disrupting in-flight
//!   generation. Emergency removal uses the PreemptionPolicy instead.
//!
//! All weights are runtime-configurable via `EvictionConfig`, loaded from
//! config/kv.toml. This allows tuning via the simulation framework.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::kv::sequence::SequenceMeta;
use crate::types::{Priority, SequenceId};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Eviction scoring weights. All values are configurable at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionConfig {
    /// Weight for idle time component. Higher = idle sequences evicted faster.
    #[serde(default = "default_idle_weight")]
    pub idle_weight: f64,

    /// Weight for size component. Higher = larger sequences evicted first.
    #[serde(default = "default_size_weight")]
    pub size_weight: f64,

    /// Weight for reuse prediction. Higher = high-reuse sequences protected more.
    #[serde(default = "default_reuse_weight")]
    pub reuse_weight: f64,

    /// Alpha coefficient for reuse prediction (request frequency component).
    #[serde(default = "default_reuse_alpha")]
    pub reuse_alpha: f64,

    /// Beta coefficient for reuse prediction (recency component).
    #[serde(default = "default_reuse_beta")]
    pub reuse_beta: f64,

    /// Maximum idle time in seconds for normalization. Idle times above this
    /// are clamped to 1.0 in the normalized score.
    #[serde(default = "default_max_idle_secs")]
    pub max_idle_secs: f64,

    /// Maximum single-sequence size in bytes for normalization.
    /// Sequences at or above this size score 1.0 for the size component.
    #[serde(default = "default_max_seq_bytes")]
    pub max_sequence_bytes: u64,

    /// Weight for batch contribution. Active batch members get
    /// a protection bonus that reduces their eviction score. Higher = more
    /// protection for sequences currently generating tokens.
    #[serde(default = "default_batch_weight")]
    pub batch_weight: f64,
}

fn default_idle_weight() -> f64 {
    1.0
}
fn default_size_weight() -> f64 {
    0.5
}
fn default_reuse_weight() -> f64 {
    0.8
}
fn default_reuse_alpha() -> f64 {
    0.6
}
fn default_reuse_beta() -> f64 {
    0.4
}
fn default_max_idle_secs() -> f64 {
    600.0 // 10 minutes
}
fn default_max_seq_bytes() -> u64 {
    2_000_000_000 // 2 GB
}
fn default_batch_weight() -> f64 {
    100.0 // Strong protection for active batch members
}

impl Default for EvictionConfig {
    fn default() -> Self {
        Self {
            idle_weight: default_idle_weight(),
            size_weight: default_size_weight(),
            reuse_weight: default_reuse_weight(),
            reuse_alpha: default_reuse_alpha(),
            reuse_beta: default_reuse_beta(),
            max_idle_secs: default_max_idle_secs(),
            max_sequence_bytes: default_max_seq_bytes(),
            batch_weight: default_batch_weight(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// The eviction scorer. Computes a score for a sequence based on the
/// configured weights. Higher score = more likely to be evicted.
#[derive(Debug, Clone)]
pub struct EvictionScorer {
    config: EvictionConfig,
}

impl EvictionScorer {
    /// Create a new scorer with the given configuration.
    pub fn new(config: EvictionConfig) -> Self {
        Self { config }
    }

    /// Update the scoring configuration at runtime.
    pub fn update_config(&mut self, config: EvictionConfig) {
        self.config = config;
    }

    /// Current configuration (for introspection / testing).
    pub fn config(&self) -> &EvictionConfig {
        &self.config
    }

    /// Compute the eviction score for a sequence.
    ///
    /// Higher score = more evictable.
    ///
    /// Score formula:
    ///   score = (idle_weight * idle_normalized)
    ///         + (size_weight * size_normalized)
    ///         + priority_penalty
    ///         - (reuse_weight * reuse_score)
    ///
    /// This variant does not account for batch membership. Use
    /// `score_with_batch()` when batch state is available.
    pub fn score(&self, meta: &SequenceMeta) -> f64 {
        self.score_inner(meta, false)
    }

    /// Compute the eviction score with batch membership awareness.
    ///
    /// If `is_in_batch` is true, the sequence receives a protection bonus
    /// equal to `batch_weight`, making it much less likely to be evicted.
    /// Active batch members should almost never be evicted; use the
    /// preemption policy for emergency removal instead.
    pub fn score_with_batch(&self, meta: &SequenceMeta, is_in_batch: bool) -> f64 {
        self.score_inner(meta, is_in_batch)
    }

    /// Score sequences with a set of active batch member IDs.
    ///
    /// Convenience method for bulk scoring: sequences whose IDs appear in
    /// `active_batch_ids` receive the batch protection bonus.
    pub fn score_batch_aware(
        &self,
        meta: &SequenceMeta,
        active_batch_ids: &HashSet<SequenceId>,
    ) -> f64 {
        let in_batch = active_batch_ids.contains(&meta.seq_id);
        self.score_inner(meta, in_batch)
    }

    /// Inner scoring implementation.
    fn score_inner(&self, meta: &SequenceMeta, is_in_batch: bool) -> f64 {
        let idle = self.idle_component(meta);
        let size = self.size_component(meta);
        let priority = self.priority_penalty(meta.priority);
        let reuse = self.reuse_score(meta);

        // Batch contribution: active batch members get a protection bonus
        // that makes them very unlikely to be evicted through normal eviction.
        // Emergency preemption handles the case
        // where active members MUST be removed.
        let batch_contribution = if is_in_batch {
            self.config.batch_weight
        } else {
            0.0
        };

        (self.config.idle_weight * idle) + (self.config.size_weight * size) + priority
            - (self.config.reuse_weight * reuse)
            - batch_contribution
    }

    /// Idle time component: normalized to [0.0, 1.0].
    /// idle_normalized = min(idle_secs / max_idle_secs, 1.0)
    fn idle_component(&self, meta: &SequenceMeta) -> f64 {
        let idle_secs = meta.idle_time().as_secs_f64();
        (idle_secs / self.config.max_idle_secs).min(1.0)
    }

    /// Size component: normalized to [0.0, 1.0].
    /// size_normalized = min(kv_bytes / max_sequence_bytes, 1.0)
    #[allow(clippy::cast_precision_loss)] // Acceptable: byte counts don't need full u64 precision in ratio
    fn size_component(&self, meta: &SequenceMeta) -> f64 {
        if self.config.max_sequence_bytes == 0 {
            return 0.0;
        }
        (meta.kv_bytes as f64 / self.config.max_sequence_bytes as f64).min(1.0)
    }

    /// Priority penalty. System priority gets a massive negative penalty
    /// (effectively preventing eviction). Others get graduated offsets.
    #[allow(clippy::unused_self)] // Method on scorer for API consistency
    fn priority_penalty(&self, priority: Priority) -> f64 {
        match priority {
            Priority::System => -1000.0,
            Priority::High => -50.0,
            Priority::Normal => 0.0,
            Priority::Background => 50.0,
        }
    }

    /// Reuse prediction score. Higher = more likely to be reused (protect it).
    ///
    /// reuse_score = alpha * (request_count / session_age_minutes)
    ///             + beta * (1.0 / idle_time_seconds.max(1.0))
    ///
    /// High request frequency + recent activity = high reuse likelihood.
    ///
    /// `age_minutes` is floored at 1.0 to prevent near-zero ages from
    /// producing unbounded frequency values that overwhelm idle, size,
    /// and priority components.
    fn reuse_score(&self, meta: &SequenceMeta) -> f64 {
        let age_minutes = (meta.age().as_secs_f64() / 60.0).max(1.0);
        let frequency = f64::from(meta.request_count) / age_minutes;

        let idle_secs = meta.idle_time().as_secs_f64().max(1.0);
        let recency = 1.0 / idle_secs;

        self.config.reuse_alpha * frequency + self.config.reuse_beta * recency
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::sequence::{ModelMemoryFactor, SequenceMeta};
    use crate::types::{InstanceId, SequenceId};
    use std::thread;
    use std::time::Duration;

    fn test_factor() -> ModelMemoryFactor {
        ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        }
    }

    fn make_meta(tokens: usize, priority: Priority) -> SequenceMeta {
        SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("test:0"),
            "llama3-8b".to_string(),
            tokens,
            priority,
            &test_factor(),
        )
    }

    #[test]
    fn test_idle_sequence_scores_higher() {
        let scorer = EvictionScorer::new(EvictionConfig::default());

        // Create both at the same time so age is equal
        let idle = make_meta(512, Priority::Normal);
        let mut fresh = make_meta(512, Priority::Normal);

        // Let both age, then touch fresh to give it higher request count
        thread::sleep(Duration::from_millis(50));
        fresh.record_request();

        let idle_score = scorer.score(&idle);
        let fresh_score = scorer.score(&fresh);

        // Idle sequence has fewer requests (lower reuse prediction),
        // so it should score higher (more evictable).
        assert!(
            idle_score >= fresh_score,
            "idle ({idle_score}) should score >= fresh ({fresh_score})"
        );
    }

    #[test]
    fn test_large_sequence_scores_higher() {
        let scorer = EvictionScorer::new(EvictionConfig::default());

        let small = make_meta(128, Priority::Normal);
        let large = make_meta(4096, Priority::Normal);

        let small_score = scorer.score(&small);
        let large_score = scorer.score(&large);

        // Larger sequence should have higher size component
        assert!(
            large_score > small_score,
            "large ({large_score}) should score > small ({small_score})"
        );
    }

    #[test]
    fn test_system_priority_never_evicted() {
        let scorer = EvictionScorer::new(EvictionConfig::default());

        let system = make_meta(4096, Priority::System);
        let background = make_meta(128, Priority::Background);

        let system_score = scorer.score(&system);
        let bg_score = scorer.score(&background);

        // System priority should have a massively negative score
        assert!(
            system_score < 0.0,
            "system score ({system_score}) should be negative"
        );
        assert!(
            system_score < bg_score,
            "system ({system_score}) should be far below background ({bg_score})"
        );
    }

    #[test]
    fn test_background_priority_evicted_first() {
        let scorer = EvictionScorer::new(EvictionConfig::default());

        let normal = make_meta(512, Priority::Normal);
        let background = make_meta(512, Priority::Background);

        let normal_score = scorer.score(&normal);
        let bg_score = scorer.score(&background);

        // Background gets +50 penalty, normal gets 0
        assert!(
            bg_score > normal_score,
            "background ({bg_score}) should score > normal ({normal_score})"
        );
    }

    #[test]
    fn test_frequent_requester_protected() {
        let scorer = EvictionScorer::new(EvictionConfig::default());

        let mut frequent = make_meta(512, Priority::Normal);
        // Simulate many requests
        for _ in 0..20 {
            frequent.record_request();
        }

        let infrequent = make_meta(512, Priority::Normal);

        let freq_score = scorer.score(&frequent);
        let infreq_score = scorer.score(&infrequent);

        // Frequent requester has higher reuse score, so lower eviction score
        assert!(
            freq_score < infreq_score,
            "frequent ({freq_score}) should score < infrequent ({infreq_score})"
        );
    }

    #[test]
    fn test_priority_ordering() {
        let scorer = EvictionScorer::new(EvictionConfig::default());
        let system = scorer.priority_penalty(Priority::System);
        let high = scorer.priority_penalty(Priority::High);
        let normal = scorer.priority_penalty(Priority::Normal);
        let background = scorer.priority_penalty(Priority::Background);

        assert!(system < high);
        assert!(high < normal);
        assert!(normal < background);
    }

    #[test]
    fn test_reuse_score_with_zero_age() {
        // Edge case: sequence just created, age is essentially zero
        let scorer = EvictionScorer::new(EvictionConfig::default());
        let meta = make_meta(512, Priority::Normal);

        // Should not panic or return NaN
        let reuse = scorer.reuse_score(&meta);
        assert!(reuse.is_finite(), "reuse score should be finite: {reuse}");
    }

    #[test]
    fn test_config_update() {
        let mut scorer = EvictionScorer::new(EvictionConfig::default());
        assert!((scorer.config().idle_weight - 1.0).abs() < f64::EPSILON);

        let new_config = EvictionConfig {
            idle_weight: 2.0,
            ..EvictionConfig::default()
        };
        scorer.update_config(new_config);
        assert!((scorer.config().idle_weight - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_batch_member_protected() {
        let scorer = EvictionScorer::new(EvictionConfig::default());
        let meta = make_meta(512, Priority::Normal);

        let score_no_batch = scorer.score(&meta);
        let score_in_batch = scorer.score_with_batch(&meta, true);

        // Active batch member should have much lower eviction score
        assert!(
            score_in_batch < score_no_batch,
            "in-batch ({score_in_batch}) should score < not-in-batch ({score_no_batch})"
        );
        // The difference should be approximately batch_weight (100.0)
        let diff = score_no_batch - score_in_batch;
        assert!(
            (diff - 100.0).abs() < 0.001,
            "difference ({diff}) should be ~100.0"
        );
    }

    #[test]
    fn test_batch_aware_scoring_with_set() {
        let scorer = EvictionScorer::new(EvictionConfig::default());
        let meta = make_meta(512, Priority::Normal);

        let mut active_ids = std::collections::HashSet::new();
        active_ids.insert(meta.seq_id);

        let score_active = scorer.score_batch_aware(&meta, &active_ids);
        let score_inactive = scorer.score_batch_aware(&meta, &std::collections::HashSet::new());

        assert!(score_active < score_inactive);
    }

    #[test]
    fn test_size_component_zero_max() {
        let config = EvictionConfig {
            max_sequence_bytes: 0,
            ..EvictionConfig::default()
        };
        let scorer = EvictionScorer::new(config);
        let meta = make_meta(512, Priority::Normal);

        // Should return 0.0, not panic on division by zero
        let score = scorer.score(&meta);
        assert!(score.is_finite());
    }
}
