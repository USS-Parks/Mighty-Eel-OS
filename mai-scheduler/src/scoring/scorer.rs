//! Multi-factor scoring orchestrator.
//!
//! Combines latency, memory pressure, topology cost, eviction cost, and
//! batching benefit into a single composite score for each candidate instance.
//! This scorer replaces `least_loaded_scorer` as the default
//! placement function.
//!
//! # Score Formula
//!
//! ```text
//! Score = latency_weight   * latency_penalty
//!       + memory_weight    * memory_penalty
//!       + topology_weight  * topology_penalty
//!       + eviction_weight  * eviction_penalty
//!       - batching_weight  * batching_benefit
//!       - continuation_bonus  (if warm KV cache hit)
//! ```
//!
//! Lower score = better candidate.
//!
//! All sub-scores are normalized to `[0.0, 1.0]` before weighting, so
//! weights are directly comparable: a latency_weight of 2.0 means latency
//! matters twice as much as a component with weight 1.0.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::kv::manager::KvCacheManager;
use crate::scoring::batching::{BatchBenefitConfig, batching_benefit};
use crate::scoring::eviction_cost::{EvictionCostConfig, eviction_penalty};
use crate::scoring::latency::{LatencyConfig, latency_penalty};
use crate::scoring::memory::{MemoryConfig, memory_penalty};
use crate::scoring::topology_score::{TopologyScoreConfig, topology_penalty};
use crate::topology::GpuTopology;
use crate::types::{InstanceState, ScheduleRequest, ScoringFn, ScoringReasonFn};

// ---------------------------------------------------------------------------
// Score breakdown (debug output)
// ---------------------------------------------------------------------------

/// Detailed breakdown of how the composite score was computed.
/// Included in `ScheduleDecision.placement_reason` for debugging and tuning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// Raw latency sub-score (0.0..1.0).
    pub latency: f64,
    /// Raw memory pressure sub-score (0.0..1.0).
    pub memory: f64,
    /// Raw topology sub-score (0.0..1.0).
    pub topology: f64,
    /// Raw eviction cost sub-score (0.0..1.0).
    pub eviction: f64,
    /// Raw batching benefit sub-score (0.0..1.0), subtracted from total.
    pub batch: f64,
    /// Whether a continuation KV cache hit was detected.
    pub continuation_hit: bool,
    /// Final composite score (lower = better).
    pub total: f64,
}

impl ScoreBreakdown {
    /// Format the breakdown as a compact debug string.
    pub fn to_reason_string(&self) -> String {
        format!(
            "multi-factor(lat={:.3} mem={:.3} topo={:.3} evict={:.3} batch=-{:.3} cont={} total={:.3})",
            self.latency,
            self.memory,
            self.topology,
            self.eviction,
            self.batch,
            self.continuation_hit,
            self.total
        )
    }
}

impl std::fmt::Display for ScoreBreakdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_reason_string())
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Top-level scoring configuration. All weights and sub-scorer configs.
/// Loaded from config/scoring.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfig {
    /// Weight for latency penalty. Higher = latency matters more.
    /// Default: 2.0 (latency is the most user-visible factor).
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f64,

    /// Weight for memory pressure penalty. Default: 1.5.
    #[serde(default = "default_memory_weight")]
    pub memory_weight: f64,

    /// Weight for topology penalty (multi-GPU placement quality).
    /// Default: 1.0. Increase for tensor-parallel-heavy workloads.
    #[serde(default = "default_topology_weight")]
    pub topology_weight: f64,

    /// Weight for eviction cost penalty. Default: 1.0.
    #[serde(default = "default_eviction_weight")]
    pub eviction_weight: f64,

    /// Weight for batching benefit (subtracted from score). Default: 1.5.
    /// Higher = prefer instances with batch headroom.
    #[serde(default = "default_batching_weight")]
    pub batching_weight: f64,

    /// Bonus subtracted when a continuation request finds a warm KV cache.
    /// This is NOT normalized; it's an absolute bonus. A large value means
    /// KV cache hits dominate all other factors. Default: 10.0.
    #[serde(default = "default_continuation_bonus")]
    pub continuation_bonus: f64,

    /// Latency sub-scorer configuration.
    #[serde(default)]
    pub latency: LatencyConfig,

    /// Memory sub-scorer configuration.
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Topology sub-scorer configuration.
    #[serde(default)]
    pub topology: TopologyScoreConfig,

    /// Eviction cost sub-scorer configuration.
    #[serde(default)]
    pub eviction: EvictionCostConfig,

    /// Batching benefit sub-scorer configuration.
    #[serde(default)]
    pub batching: BatchBenefitConfig,
}

fn default_latency_weight() -> f64 {
    2.0
}
fn default_memory_weight() -> f64 {
    1.5
}
fn default_topology_weight() -> f64 {
    1.0
}
fn default_eviction_weight() -> f64 {
    1.0
}
fn default_batching_weight() -> f64 {
    1.5
}
fn default_continuation_bonus() -> f64 {
    10.0
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            latency_weight: default_latency_weight(),
            memory_weight: default_memory_weight(),
            topology_weight: default_topology_weight(),
            eviction_weight: default_eviction_weight(),
            batching_weight: default_batching_weight(),
            continuation_bonus: default_continuation_bonus(),
            latency: LatencyConfig::default(),
            memory: MemoryConfig::default(),
            topology: TopologyScoreConfig::default(),
            eviction: EvictionCostConfig::default(),
            batching: BatchBenefitConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-factor scorer
// ---------------------------------------------------------------------------

/// The multi-factor scorer. Holds references to shared subsystems
/// (topology, KV cache manager) and the scoring configuration.
///
/// This struct is NOT stored directly. Instead, `build_scoring_fn()` captures
/// the scorer in a closure that matches the `ScoringFn` type signature. This
/// preserves backward compatibility with PlacementEngine's pluggable scorer.
pub struct MultiFactorScorer {
    config: ScoringConfig,
    topology: Option<Arc<GpuTopology>>,
    kv_manager: Option<Arc<dyn KvCacheManager>>,
}

impl MultiFactorScorer {
    /// Create a new multi-factor scorer.
    pub fn new(config: ScoringConfig) -> Self {
        Self {
            config,
            topology: None,
            kv_manager: None,
        }
    }

    /// Attach GPU topology for topology-aware scoring.
    pub fn with_topology(mut self, topology: Arc<GpuTopology>) -> Self {
        self.topology = Some(topology);
        self
    }

    /// Attach KV cache manager for eviction-aware scoring.
    pub fn with_kv_manager(mut self, kv_manager: Arc<dyn KvCacheManager>) -> Self {
        self.kv_manager = Some(kv_manager);
        self
    }

    /// Score a candidate instance for a given request.
    ///
    /// Returns the composite score and a detailed breakdown.
    pub fn score(&self, state: &InstanceState, request: &ScheduleRequest) -> (f64, ScoreBreakdown) {
        let lat = latency_penalty(state, &self.config.latency);
        let mem = memory_penalty(state, &self.config.memory);
        let topo = topology_penalty(state, self.topology.as_ref(), &self.config.topology);
        let evict = eviction_penalty(
            state,
            request,
            self.kv_manager.as_ref(),
            &self.config.eviction,
        );
        let batch = batching_benefit(state, &self.config.batching);

        // Check for continuation KV cache hit
        let continuation_hit = self.check_continuation(state, request);

        let total = self.config.latency_weight * lat
            + self.config.memory_weight * mem
            + self.config.topology_weight * topo
            + self.config.eviction_weight * evict
            - self.config.batching_weight * batch
            - if continuation_hit {
                self.config.continuation_bonus
            } else {
                0.0
            };

        let breakdown = ScoreBreakdown {
            latency: lat,
            memory: mem,
            topology: topo,
            eviction: evict,
            batch,
            continuation_hit,
            total,
        };

        (total, breakdown)
    }

    /// Check whether this instance has a warm KV cache for the continuation
    /// sequence. A warm cache hit means re-prefill is skipped entirely.
    fn check_continuation(&self, state: &InstanceState, request: &ScheduleRequest) -> bool {
        let continuation_seq = match &request.continuation_of {
            Some(seq) => seq,
            None => return false,
        };

        // First check: does the instance's last_sequence_id match?
        if let Some(ref last_seq) = state.metrics.last_sequence_id
            && last_seq == continuation_seq
        {
            // Second check: is the sequence still in the KV cache?
            if let Some(ref kv) = self.kv_manager {
                return kv.sequence_meta(*continuation_seq).is_some();
            }
            // No KV manager: rely on the instance-level hint
            return true;
        }

        false
    }

    /// Build a `ScoringFn` closure that captures this scorer.
    ///
    /// The returned closure has the signature
    /// `Fn(&InstanceState, &ScheduleRequest) -> f64`
    /// which is compatible with `PlacementEngine::set_scorer()`.
    ///
    /// Note: the closure returns only the composite score (f64), not the
    /// breakdown. The breakdown is available via `score()` for callers that
    /// need it (e.g., the enhanced placement engine).
    pub fn into_scoring_fn(self) -> ScoringFn {
        let scorer = Arc::new(self);
        Box::new(move |state: &InstanceState, request: &ScheduleRequest| {
            let (total, _breakdown) = scorer.score(state, request);
            total
        })
    }

    /// Build scoring and diagnostic closures that share the same scorer.
    pub fn into_scoring_parts(self) -> (ScoringFn, ScoringReasonFn) {
        let scorer = Arc::new(self);
        let scoring_scorer = Arc::clone(&scorer);
        let scoring_fn: ScoringFn =
            Box::new(move |state: &InstanceState, request: &ScheduleRequest| {
                let (total, _breakdown) = scoring_scorer.score(state, request);
                total
            });
        let reason_fn: ScoringReasonFn =
            Box::new(move |state: &InstanceState, request: &ScheduleRequest| {
                let (_total, breakdown) = scorer.score(state, request);
                breakdown.to_reason_string()
            });
        (scoring_fn, reason_fn)
    }
}

// ---------------------------------------------------------------------------
// Convenience: build a scorer from components
// ---------------------------------------------------------------------------

/// Build a multi-factor scoring function from the provided components.
///
/// This is the main entry point for wiring the scorer into DefaultScheduler.
pub fn build_multi_factor_scorer(
    config: ScoringConfig,
    topology: Option<Arc<GpuTopology>>,
    kv_manager: Option<Arc<dyn KvCacheManager>>,
) -> ScoringFn {
    let mut scorer = MultiFactorScorer::new(config);
    if let Some(topo) = topology {
        scorer = scorer.with_topology(topo);
    }
    if let Some(kv) = kv_manager {
        scorer = scorer.with_kv_manager(kv);
    }
    scorer.into_scoring_fn()
}

/// Build a scoring function and a matching placement-reason formatter.
pub fn build_multi_factor_scorer_with_reason(
    config: ScoringConfig,
    topology: Option<Arc<GpuTopology>>,
    kv_manager: Option<Arc<dyn KvCacheManager>>,
) -> (ScoringFn, ScoringReasonFn) {
    let mut scorer = MultiFactorScorer::new(config);
    if let Some(topo) = topology {
        scorer = scorer.with_topology(topo);
    }
    if let Some(kv) = kv_manager {
        scorer = scorer.with_kv_manager(kv);
    }
    scorer.into_scoring_parts()
}

/// Build a multi-factor scorer and return the Arc'd scorer itself (not just
/// the closure). Useful when callers need both the ScoringFn and direct
/// access to `score()` for breakdowns.
pub fn build_scorer(
    config: ScoringConfig,
    topology: Option<Arc<GpuTopology>>,
    kv_manager: Option<Arc<dyn KvCacheManager>>,
) -> Arc<MultiFactorScorer> {
    let mut scorer = MultiFactorScorer::new(config);
    if let Some(topo) = topology {
        scorer = scorer.with_topology(topo);
    }
    if let Some(kv) = kv_manager {
        scorer = scorer.with_kv_manager(kv);
    }
    Arc::new(scorer)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics, InstanceState,
        Priority, ScheduleRequest, SequenceId,
    };

    fn make_state_full(
        queue_depth: u32,
        vram_used: u64,
        vram_allocated: u64,
        batch_utilization: f64,
        batch_waiting: u32,
        decode_slots: u32,
        gpu_ids: Vec<GpuId>,
    ) -> InstanceState {
        InstanceState {
            config: InstanceConfig {
                id: InstanceId::new("test:0"),
                model_name: "test-model".to_string(),
                adapter_type: "test".to_string(),
                gpu_ids,
                max_batch_size: 16,
                vram_allocated,
                capabilities: InstanceCapabilities::default(),
            },
            metrics: InstanceMetrics {
                queue_depth,
                active_sequences: queue_depth,
                vram_used,
                batch_utilization,
                batch_waiting_count: batch_waiting,
                decode_slots_used: decode_slots,
                ..InstanceMetrics::default()
            },
        }
    }

    fn make_request() -> ScheduleRequest {
        ScheduleRequest::new("test-model", Priority::Normal)
    }

    #[test]
    fn test_empty_instance_low_score() {
        let scorer = MultiFactorScorer::new(ScoringConfig::default());
        let state = make_state_full(0, 0, 16_000_000_000, 0.0, 0, 0, vec![GpuId(0)]);
        let (score, breakdown) = scorer.score(&state, &make_request());

        // Empty instance: latency=0, memory=0, topo=0, evict=0, batch=1.0 (max benefit)
        // score = -1.5 * 1.0 = -1.5
        assert!(
            score < 0.0,
            "empty instance should have negative score (batch benefit), got {score}"
        );
        assert!(breakdown.latency.abs() < f64::EPSILON);
        assert!(breakdown.memory.abs() < f64::EPSILON);
        assert!(!breakdown.continuation_hit);
    }

    #[test]
    fn test_loaded_instance_higher_score() {
        let scorer = MultiFactorScorer::new(ScoringConfig::default());

        let empty = make_state_full(0, 0, 16_000_000_000, 0.0, 0, 0, vec![GpuId(0)]);
        let loaded = make_state_full(
            10,
            12_000_000_000,
            16_000_000_000,
            0.8,
            5,
            8,
            vec![GpuId(0)],
        );

        let (empty_score, _) = scorer.score(&empty, &make_request());
        let (loaded_score, _) = scorer.score(&loaded, &make_request());

        assert!(
            loaded_score > empty_score,
            "loaded instance ({loaded_score}) should score higher than empty ({empty_score})"
        );
    }

    #[test]
    fn test_score_breakdown_populated() {
        let scorer = MultiFactorScorer::new(ScoringConfig::default());
        let state = make_state_full(5, 8_000_000_000, 16_000_000_000, 0.5, 2, 4, vec![GpuId(0)]);
        let (_score, breakdown) = scorer.score(&state, &make_request());

        assert!(breakdown.latency > 0.0);
        assert!(breakdown.memory > 0.0);
        assert!(breakdown.batch > 0.0);
        assert!(!breakdown.continuation_hit);
        let reason = breakdown.to_reason_string();
        assert!(reason.starts_with("multi-factor("));
    }

    #[test]
    fn test_weight_changes_affect_outcome() {
        let state_a = make_state_full(10, 4_000_000_000, 16_000_000_000, 0.2, 0, 0, vec![GpuId(0)]);
        let state_b = make_state_full(2, 14_000_000_000, 16_000_000_000, 0.8, 0, 0, vec![GpuId(0)]);

        // Default weights: latency_weight=2.0, memory_weight=1.5
        // A has high latency, low memory. B has low latency, high memory.
        let default_config = ScoringConfig::default();
        let scorer = MultiFactorScorer::new(default_config);
        let (score_a_default, _) = scorer.score(&state_a, &make_request());
        let (score_b_default, _) = scorer.score(&state_b, &make_request());

        // Now flip: make memory weight very high, latency weight very low
        let flipped = ScoringConfig {
            latency_weight: 0.1,
            memory_weight: 10.0,
            ..ScoringConfig::default()
        };
        let scorer_flipped = MultiFactorScorer::new(flipped);
        let (score_a_flipped, _) = scorer_flipped.score(&state_a, &make_request());
        let (score_b_flipped, _) = scorer_flipped.score(&state_b, &make_request());

        // With high memory weight, B (high memory) should now be worse relative to A
        let diff_default = score_a_default - score_b_default;
        let diff_flipped = score_a_flipped - score_b_flipped;
        assert!(
            diff_flipped < diff_default,
            "flipping weights should change relative scores"
        );
    }

    #[test]
    fn test_all_zero_weights_equal_scores() {
        let config = ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        };
        let scorer = MultiFactorScorer::new(config);

        let state_a = make_state_full(1, 1_000_000_000, 16_000_000_000, 0.1, 0, 0, vec![GpuId(0)]);
        let state_b = make_state_full(
            10,
            14_000_000_000,
            16_000_000_000,
            0.9,
            5,
            8,
            vec![GpuId(0)],
        );

        let (score_a, _) = scorer.score(&state_a, &make_request());
        let (score_b, _) = scorer.score(&state_b, &make_request());

        assert!(
            (score_a - score_b).abs() < f64::EPSILON,
            "all-zero weights should produce equal scores: a={score_a}, b={score_b}"
        );
    }

    #[test]
    fn test_continuation_bonus_applied() {
        let config = ScoringConfig {
            continuation_bonus: 100.0,
            ..ScoringConfig::default()
        };

        let seq_id = SequenceId::new();
        let mut state =
            make_state_full(5, 8_000_000_000, 16_000_000_000, 0.5, 0, 0, vec![GpuId(0)]);
        state.metrics.last_sequence_id = Some(seq_id);

        // Without continuation
        let scorer = MultiFactorScorer::new(config.clone());
        let (score_no_cont, breakdown_no) = scorer.score(&state, &make_request());
        assert!(!breakdown_no.continuation_hit);

        // With continuation pointing at this instance
        let mut req = make_request();
        req.continuation_of = Some(seq_id);
        let (score_cont, breakdown_cont) = scorer.score(&state, &req);
        assert!(breakdown_cont.continuation_hit);
        assert!(
            score_cont < score_no_cont,
            "continuation hit should reduce score: cont={score_cont}, no_cont={score_no_cont}"
        );
        assert!(
            (score_no_cont - score_cont - 100.0).abs() < 0.01,
            "continuation bonus should be exactly 100.0"
        );
    }

    #[test]
    fn test_continuation_no_match_no_bonus() {
        let config = ScoringConfig {
            continuation_bonus: 100.0,
            ..ScoringConfig::default()
        };
        let scorer = MultiFactorScorer::new(config);

        let state = make_state_full(5, 8_000_000_000, 16_000_000_000, 0.5, 0, 0, vec![GpuId(0)]);

        // continuation_of set to a different sequence
        let mut req = make_request();
        req.continuation_of = Some(SequenceId::new());
        let (_score, breakdown) = scorer.score(&state, &req);
        assert!(!breakdown.continuation_hit);
    }

    #[test]
    fn test_into_scoring_fn() {
        let scorer = MultiFactorScorer::new(ScoringConfig::default());
        let scoring_fn = scorer.into_scoring_fn();

        let state = make_state_full(5, 8_000_000_000, 16_000_000_000, 0.5, 0, 0, vec![GpuId(0)]);
        let req = make_request();
        let score = scoring_fn(&state, &req);
        assert!(score.is_finite());
    }

    #[test]
    fn test_build_multi_factor_scorer_function() {
        let scoring_fn = build_multi_factor_scorer(ScoringConfig::default(), None, None);
        let state = make_state_full(0, 0, 16_000_000_000, 0.0, 0, 0, vec![GpuId(0)]);
        let score = scoring_fn(&state, &make_request());
        assert!(score.is_finite());
    }
}
