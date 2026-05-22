//! KV cache management subsystem.
//!
//! Manages GPU VRAM used for KV caches across all active inference sequences.
//! The `HeuristicKvCacheManager` is the production implementation of the
//! `KvCacheManager` trait, composing:
//!
//! - `SequenceMeta` for per-sequence tracking
//! - `EvictionScorer` for multi-factor eviction scoring
//! - `ThrashGuard` for anti-thrashing protection
//! - Trigger evaluation for threshold-based eviction
//!
//! # Thread Safety
//!
//! The manager uses `DashMap` for sequence tracking (lock-free reads) and
//! a `Mutex<ThrashGuard>` for eviction state (sequential eviction decisions).
//! `can_fit()`, `touch()`, and `sequence_meta()` are lock-free.
//! `evict()` and `eviction_candidates()` take the guard mutex briefly.

pub mod eviction;
pub mod guard;
pub mod manager;
pub mod offload;
pub mod sequence;
pub mod tiered;
pub mod triggers;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::types::{SchedulerError, SequenceId};

use self::eviction::{EvictionConfig, EvictionScorer};
use self::guard::{AntiThrashConfig, EvictionRecord, ThrashGuard};
use self::manager::KvCacheManager;
use self::sequence::{ModelMemoryFactor, SequenceMeta};
use self::triggers::{EvictionAction, TriggerConfig};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Top-level KV cache configuration, loaded from config/kv.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvCacheConfig {
    /// Total VRAM budget for KV caches in bytes.
    /// Default: 8 GB (conservative for a single GPU setup).
    #[serde(default = "default_total_budget")]
    pub total_budget_bytes: u64,

    /// Per-model memory factors. Key is backend model name (e.g., "llama3-8b").
    #[serde(default)]
    pub model_factors: HashMap<String, ModelMemoryFactor>,

    /// Eviction scoring weights.
    #[serde(default)]
    pub eviction: EvictionConfig,

    /// Anti-thrashing configuration.
    #[serde(default)]
    pub anti_thrash: AntiThrashConfig,

    /// Trigger thresholds.
    #[serde(default)]
    pub triggers: TriggerConfig,
}

fn default_total_budget() -> u64 {
    8_000_000_000 // 8 GB
}

impl Default for KvCacheConfig {
    fn default() -> Self {
        Self {
            total_budget_bytes: default_total_budget(),
            model_factors: HashMap::new(),
            eviction: EvictionConfig::default(),
            anti_thrash: AntiThrashConfig::default(),
            triggers: TriggerConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// HeuristicKvCacheManager
// ---------------------------------------------------------------------------

/// Production implementation of the KV cache manager.
///
/// Uses heuristic scoring for eviction decisions. All weights are
/// configurable and tunable at runtime via config reload.
pub struct HeuristicKvCacheManager {
    /// Active sequences, keyed by SequenceId. DashMap for concurrent access.
    sequences: DashMap<SequenceId, SequenceMeta>,

    /// Total VRAM budget for KV caches.
    total_budget: u64,

    /// Current bytes allocated (atomic for lock-free reads).
    used_bytes: AtomicU64,

    /// Eviction scorer (immutable config, cloned per scoring pass).
    scorer: EvictionScorer,

    /// Anti-thrashing guard. Mutex-protected because eviction decisions
    /// are inherently sequential.
    guard: Mutex<ThrashGuard>,

    /// Trigger configuration.
    trigger_config: TriggerConfig,

    /// Per-model memory factors for KV size estimation.
    model_factors: HashMap<String, ModelMemoryFactor>,

    /// Fallback memory factor for unknown models.
    fallback_factor: ModelMemoryFactor,
}

impl HeuristicKvCacheManager {
    /// Create a new manager from configuration.
    pub fn new(config: KvCacheConfig) -> Self {
        let scorer = EvictionScorer::new(config.eviction.clone());
        let guard = ThrashGuard::new(config.anti_thrash.clone());

        #[allow(clippy::cast_precision_loss)] // Acceptable: display-only metric
        let budget_gb = config.total_budget_bytes as f64 / 1_000_000_000.0;
        info!(
            budget_gb = budget_gb,
            model_count = config.model_factors.len(),
            "HeuristicKvCacheManager initialized"
        );

        Self {
            sequences: DashMap::new(),
            total_budget: config.total_budget_bytes,
            used_bytes: AtomicU64::new(0),
            scorer,
            guard: Mutex::new(guard),
            trigger_config: config.triggers,
            model_factors: config.model_factors,
            fallback_factor: sequence::fallback_memory_factor(),
        }
    }

    /// Look up the memory factor for a model, falling back to the default.
    pub fn factor_for_model(&self, model_name: &str) -> &ModelMemoryFactor {
        self.model_factors
            .get(model_name)
            .unwrap_or(&self.fallback_factor)
    }

    /// Evaluate triggers and return the action to take.
    pub fn evaluate_triggers(&self) -> EvictionAction {
        triggers::evaluate_triggers(
            self.used_bytes.load(Ordering::Relaxed),
            self.total_budget,
            &self.trigger_config,
        )
    }

    /// Perform eviction to free at least `needed_bytes`. Returns total freed.
    ///
    /// This is the main eviction loop. It:
    /// 1. Gets scored candidates
    /// 2. Filters by guard rules (unless bypass_residency)
    /// 3. Evicts in score order until enough bytes are freed
    /// 4. Respects the rate limiter
    pub fn perform_eviction(&self, needed_bytes: u64, bypass_residency: bool) -> u64 {
        let candidates = self.scored_candidates();
        if candidates.is_empty() {
            return 0;
        }

        let mut total_freed = 0_u64;
        let mut guard = self
            .guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        for (seq_id, _bytes, score) in &candidates {
            if total_freed >= needed_bytes {
                break;
            }

            // Rate limiter check
            if !guard.can_evict_now() {
                debug!("Eviction rate limit reached, stopping");
                break;
            }

            // Get the sequence metadata for guard checks
            if let Some(meta) = self.sequences.get(seq_id) {
                // Minimum residency check (unless emergency bypass)
                if !bypass_residency && guard.is_protected(&meta) {
                    debug!(seq = %seq_id, "Skipping protected sequence (min residency)");
                    continue;
                }

                let freed = meta.kv_bytes;
                let seq_id_copy = *seq_id;

                // Drop the DashMap ref before removing
                drop(meta);

                // Actually evict
                if let Some((_, _evicted_meta)) = self.sequences.remove(&seq_id_copy) {
                    self.used_bytes.fetch_sub(freed, Ordering::Relaxed);
                    total_freed += freed;

                    guard.record_eviction(EvictionRecord {
                        seq_id: seq_id_copy,
                        evicted_at: std::time::Instant::now(),
                        bytes_freed: freed,
                        score: *score,
                    });

                    debug!(
                        seq = %seq_id_copy,
                        freed_mb = freed / 1_000_000,
                        score = score,
                        "Sequence evicted"
                    );
                }
            }
        }

        if total_freed > 0 {
            info!(
                freed_mb = total_freed / 1_000_000,
                sequences_remaining = self.sequences.len(),
                "Eviction complete"
            );
        }

        total_freed
    }

    /// Get all sequences scored for eviction, sorted descending by score
    /// (highest = most evictable first). Includes anti-thrashing adjustments.
    fn scored_candidates(&self) -> Vec<(SequenceId, u64, f64)> {
        let guard = self
            .guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut candidates: Vec<(SequenceId, u64, f64)> = self
            .sequences
            .iter()
            .map(|entry| {
                let meta = entry.value();
                let base_score = self.scorer.score(meta);
                let adjustment = guard.score_adjustment(meta);
                let final_score = base_score + adjustment;
                (meta.seq_id, meta.kv_bytes, final_score)
            })
            .collect();

        // Sort descending by score (highest = evict first)
        candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

impl KvCacheManager for HeuristicKvCacheManager {
    fn allocate(&self, seq: SequenceMeta) -> Result<(), SchedulerError> {
        let seq_id = seq.seq_id;
        let bytes = seq.kv_bytes;

        // Check for duplicate
        if self.sequences.contains_key(&seq_id) {
            return Err(SchedulerError::ConfigError(format!(
                "sequence {seq_id} already tracked in KV cache"
            )));
        }

        // Insert and update budget
        self.sequences.insert(seq_id, seq);
        self.used_bytes.fetch_add(bytes, Ordering::Relaxed);

        debug!(
            seq = %seq_id,
            bytes_mb = bytes / 1_000_000,
            used_mb = self.used_bytes.load(Ordering::Relaxed) / 1_000_000,
            "KV cache allocated"
        );

        Ok(())
    }

    fn deallocate(&self, seq_id: SequenceId) {
        if let Some((_, meta)) = self.sequences.remove(&seq_id) {
            self.used_bytes.fetch_sub(meta.kv_bytes, Ordering::Relaxed);
            debug!(
                seq = %seq_id,
                freed_mb = meta.kv_bytes / 1_000_000,
                "KV cache deallocated"
            );
        }
    }

    fn can_fit(&self, estimated_tokens: usize, model_factor: f64) -> bool {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            clippy::cast_sign_loss
        )]
        let estimated_bytes = (estimated_tokens as f64 * model_factor) as u64;
        let used = self.used_bytes.load(Ordering::Relaxed);
        used + estimated_bytes <= self.total_budget
    }

    fn eviction_candidates(&self, needed_bytes: u64) -> Vec<(SequenceId, u64, f64)> {
        let all = self.scored_candidates();

        // Return enough candidates to cover needed_bytes
        let mut cumulative = 0_u64;
        let mut result = Vec::new();
        for candidate in all {
            result.push(candidate);
            cumulative += candidate.1;
            if cumulative >= needed_bytes {
                break;
            }
        }
        result
    }

    fn evict(&self, sequences: &[SequenceId]) -> u64 {
        let mut total_freed = 0_u64;

        for seq_id in sequences {
            if let Some((_, meta)) = self.sequences.remove(seq_id) {
                let freed = meta.kv_bytes;
                self.used_bytes.fetch_sub(freed, Ordering::Relaxed);
                total_freed += freed;

                debug!(seq = %seq_id, freed_mb = freed / 1_000_000, "KV cache evicted");
            }
        }

        total_freed
    }

    fn touch(&self, seq_id: SequenceId) {
        if let Some(mut entry) = self.sequences.get_mut(&seq_id) {
            entry.value_mut().touch();
        }
    }

    fn free_bytes(&self) -> u64 {
        self.total_budget
            .saturating_sub(self.used_bytes.load(Ordering::Relaxed))
    }

    fn total_bytes(&self) -> u64 {
        self.total_budget
    }

    fn active_sequences(&self) -> usize {
        self.sequences.len()
    }

    fn sequence_meta(&self, seq_id: SequenceId) -> Option<SequenceMeta> {
        self.sequences.get(&seq_id).map(|entry| (*entry).clone())
    }
}

// Allow debug printing even though DashMap doesn't impl Debug in a useful way
#[allow(clippy::missing_fields_in_debug)] // Intentionally omitting internal DashMap/Mutex state
impl std::fmt::Debug for HeuristicKvCacheManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeuristicKvCacheManager")
            .field("total_budget", &self.total_budget)
            .field("used_bytes", &self.used_bytes.load(Ordering::Relaxed))
            .field("active_sequences", &self.sequences.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InstanceId, Priority, SequenceId};

    fn test_factor() -> ModelMemoryFactor {
        ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        }
    }

    fn small_config() -> KvCacheConfig {
        let mut factors = HashMap::new();
        factors.insert("llama3-8b".to_string(), test_factor());

        KvCacheConfig {
            total_budget_bytes: 1_000_000_000, // 1 GB for testing
            model_factors: factors,
            eviction: EvictionConfig::default(),
            anti_thrash: AntiThrashConfig {
                min_residency_secs: 0.01, // 10ms for fast tests
                max_evictions_per_sec: 100,
                ..AntiThrashConfig::default()
            },
            triggers: TriggerConfig::default(),
        }
    }

    fn make_seq(tokens: usize, priority: Priority, factor: &ModelMemoryFactor) -> SequenceMeta {
        SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("test:0"),
            "llama3-8b".to_string(),
            tokens,
            priority,
            factor,
        )
    }

    #[test]
    fn test_allocate_and_deallocate() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();
        let seq = make_seq(512, Priority::Normal, &factor);
        let seq_id = seq.seq_id;
        let expected_bytes = factor.estimate_bytes(512);

        mgr.allocate(seq).unwrap();
        assert_eq!(mgr.active_sequences(), 1);
        assert_eq!(mgr.used_bytes.load(Ordering::Relaxed), expected_bytes);
        assert_eq!(mgr.free_bytes(), 1_000_000_000 - expected_bytes);

        mgr.deallocate(seq_id);
        assert_eq!(mgr.active_sequences(), 0);
        assert_eq!(mgr.used_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(mgr.free_bytes(), 1_000_000_000);
    }

    #[test]
    fn test_duplicate_allocation_rejected() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();
        let seq = make_seq(512, Priority::Normal, &factor);
        let seq_id = seq.seq_id;

        mgr.allocate(seq).unwrap();

        // Try to allocate same ID again
        let dup = SequenceMeta::new(
            seq_id,
            InstanceId::new("test:0"),
            "llama3-8b".to_string(),
            256,
            Priority::Normal,
            &factor,
        );
        assert!(mgr.allocate(dup).is_err());
    }

    #[test]
    fn test_can_fit_checks_budget() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();
        let bytes_per_token = factor.bytes_per_token() as f64;

        // 1 GB budget, check if 512 tokens fit
        assert!(mgr.can_fit(512, bytes_per_token));

        // Fill most of the budget
        let big_seq = make_seq(7000, Priority::Normal, &factor);
        mgr.allocate(big_seq).unwrap();

        // Now a large sequence should not fit
        // 7000 tokens = 7000 * 131072 = ~917 MB, remaining ~83 MB
        // 1000 tokens = 1000 * 131072 = ~131 MB - won't fit
        assert!(!mgr.can_fit(1000, bytes_per_token));

        // But a smaller one should
        assert!(mgr.can_fit(100, bytes_per_token));
    }

    #[test]
    fn test_touch_updates_sequence() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();
        let seq = make_seq(512, Priority::Normal, &factor);
        let seq_id = seq.seq_id;

        mgr.allocate(seq).unwrap();

        let before = mgr.sequence_meta(seq_id).unwrap().last_access;
        std::thread::sleep(std::time::Duration::from_millis(5));
        mgr.touch(seq_id);
        let after = mgr.sequence_meta(seq_id).unwrap().last_access;

        assert!(after > before);
    }

    #[test]
    fn test_eviction_candidates_sorted_by_score() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();

        // Allocate sequences with different priorities
        let bg = make_seq(512, Priority::Background, &factor);
        let normal = make_seq(512, Priority::Normal, &factor);
        let system = make_seq(512, Priority::System, &factor);

        mgr.allocate(bg).unwrap();
        mgr.allocate(normal).unwrap();
        mgr.allocate(system).unwrap();

        let candidates = mgr.eviction_candidates(u64::MAX);
        assert_eq!(candidates.len(), 3);

        // Background should be first (highest score = most evictable)
        // System should be last (lowest score = protected)
        let first_score = candidates[0].2;
        let last_score = candidates[candidates.len() - 1].2;
        assert!(
            first_score > last_score,
            "first ({first_score}) should score higher than last ({last_score})"
        );
    }

    #[test]
    fn test_evict_frees_memory() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();

        let seq1 = make_seq(512, Priority::Normal, &factor);
        let seq2 = make_seq(256, Priority::Normal, &factor);
        let id1 = seq1.seq_id;
        let id2 = seq2.seq_id;
        let bytes1 = seq1.kv_bytes;
        let bytes2 = seq2.kv_bytes;

        mgr.allocate(seq1).unwrap();
        mgr.allocate(seq2).unwrap();
        assert_eq!(mgr.active_sequences(), 2);

        let freed = mgr.evict(&[id1]);
        assert_eq!(freed, bytes1);
        assert_eq!(mgr.active_sequences(), 1);

        let freed = mgr.evict(&[id2]);
        assert_eq!(freed, bytes2);
        assert_eq!(mgr.active_sequences(), 0);
        assert_eq!(mgr.free_bytes(), 1_000_000_000);
    }

    #[test]
    fn test_evict_unknown_sequence_returns_zero() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let freed = mgr.evict(&[SequenceId::new()]);
        assert_eq!(freed, 0);
    }

    #[test]
    fn test_system_priority_resists_eviction() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();

        let system = make_seq(512, Priority::System, &factor);
        let bg = make_seq(512, Priority::Background, &factor);
        let system_id = system.seq_id;
        let bg_id = bg.seq_id;

        mgr.allocate(system).unwrap();
        mgr.allocate(bg).unwrap();

        // Request enough bytes to return both candidates
        let candidates = mgr.eviction_candidates(factor.estimate_bytes(512) * 2);

        assert_eq!(candidates.len(), 2, "should return both candidates");
        // Background should come first (highest score = most evictable)
        assert_eq!(candidates[0].0, bg_id);
        // System should be last
        assert_eq!(candidates[1].0, system_id);
        assert!(candidates[1].2 < 0.0, "system score should be negative");
    }

    #[test]
    fn test_perform_eviction_respects_residency() {
        let config = KvCacheConfig {
            total_budget_bytes: 1_000_000_000,
            anti_thrash: AntiThrashConfig {
                min_residency_secs: 60.0, // 60s - sequences are protected
                max_evictions_per_sec: 100,
                ..AntiThrashConfig::default()
            },
            ..small_config()
        };
        let mgr = HeuristicKvCacheManager::new(config);
        let factor = test_factor();

        let seq = make_seq(512, Priority::Background, &factor);
        mgr.allocate(seq).unwrap();

        // Sequence is too new (< 60s), eviction should free nothing
        let freed = mgr.perform_eviction(1_000_000, false);
        assert_eq!(freed, 0);
        assert_eq!(mgr.active_sequences(), 1);
    }

    #[test]
    fn test_perform_eviction_emergency_bypasses_residency() {
        let config = KvCacheConfig {
            total_budget_bytes: 1_000_000_000,
            anti_thrash: AntiThrashConfig {
                min_residency_secs: 60.0,
                max_evictions_per_sec: 100,
                ..AntiThrashConfig::default()
            },
            ..small_config()
        };
        let mgr = HeuristicKvCacheManager::new(config);
        let factor = test_factor();

        let seq = make_seq(512, Priority::Background, &factor);
        let expected_bytes = seq.kv_bytes;
        mgr.allocate(seq).unwrap();

        // Emergency bypass should evict even new sequences
        let freed = mgr.perform_eviction(1, true);
        assert_eq!(freed, expected_bytes);
        assert_eq!(mgr.active_sequences(), 0);
    }

    #[test]
    fn test_deallocate_idempotent() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        // Deallocating unknown sequence is a no-op
        mgr.deallocate(SequenceId::new());
        assert_eq!(mgr.active_sequences(), 0);
        assert_eq!(mgr.free_bytes(), 1_000_000_000);
    }

    #[test]
    fn test_sequence_meta_retrieval() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let factor = test_factor();

        let seq = make_seq(1024, Priority::High, &factor);
        let seq_id = seq.seq_id;
        mgr.allocate(seq).unwrap();

        let meta = mgr.sequence_meta(seq_id).unwrap();
        assert_eq!(meta.tokens, 1024);
        assert_eq!(meta.priority, Priority::High);
        assert_eq!(meta.model_name, "llama3-8b");

        // Unknown sequence returns None
        assert!(mgr.sequence_meta(SequenceId::new()).is_none());
    }

    #[test]
    fn test_evaluate_triggers_integration() {
        let config = KvCacheConfig {
            total_budget_bytes: 100, // tiny for testing
            ..KvCacheConfig::default()
        };
        let mgr = HeuristicKvCacheManager::new(config);

        // Empty: no action
        let action = mgr.evaluate_triggers();
        assert_eq!(action, EvictionAction::None);
    }

    #[test]
    fn test_total_bytes() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        assert_eq!(mgr.total_bytes(), 1_000_000_000);
    }

    #[test]
    fn test_factor_for_model_lookup() {
        let mgr = HeuristicKvCacheManager::new(small_config());
        let known = mgr.factor_for_model("llama3-8b");
        assert_eq!(known.layers, 32);

        let unknown = mgr.factor_for_model("unknown-model");
        // Should return fallback
        assert_eq!(unknown.layers, 32);
    }
}
