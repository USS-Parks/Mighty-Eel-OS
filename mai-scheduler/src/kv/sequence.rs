//! Sequence metadata for KV cache tracking.
//!
//! Each active inference sequence consumes GPU VRAM for its KV cache. This
//! module defines the metadata we track per sequence and the memory estimation
//! formula that converts token counts into byte estimates.
//!
//! Memory formula:
//!   kv_bytes = tokens * layers * heads * head_dim * 2 * dtype_size
//!
//! The "* 2" accounts for both K and V tensors. Model-specific factors
//! (layers, heads, head_dim, dtype_size) are loaded from config/kv.toml
//! and looked up by model name.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::{InstanceId, Priority, SequenceId};

// ---------------------------------------------------------------------------
// Model memory factor (config-driven)
// ---------------------------------------------------------------------------

/// Per-model KV cache memory parameters. Loaded from config/kv.toml.
///
/// These values determine how many bytes each token consumes in the KV cache
/// for a given model architecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMemoryFactor {
    /// Number of transformer layers (e.g., 32 for LLaMA-3 8B).
    pub layers: u32,
    /// Number of KV attention heads (may differ from query heads in GQA).
    pub kv_heads: u32,
    /// Dimension per head (e.g., 128).
    pub head_dim: u32,
    /// Bytes per element (2 for fp16/bf16, 1 for fp8, 4 for fp32).
    #[serde(default = "default_dtype_size")]
    pub dtype_size: u32,
}

fn default_dtype_size() -> u32 {
    2 // fp16/bf16
}

impl ModelMemoryFactor {
    /// Estimate KV cache bytes for a given token count.
    ///
    /// Formula: tokens * layers * kv_heads * head_dim * 2 * dtype_size
    /// The factor of 2 accounts for both K and V tensors.
    #[allow(clippy::cast_possible_truncation)] // usize to u64 is safe on 64-bit
    pub fn estimate_bytes(&self, tokens: usize) -> u64 {
        let per_token = u64::from(self.layers)
            * u64::from(self.kv_heads)
            * u64::from(self.head_dim)
            * 2 // K + V
            * u64::from(self.dtype_size);
        tokens as u64 * per_token
    }

    /// Bytes consumed by a single token in the KV cache.
    pub fn bytes_per_token(&self) -> u64 {
        u64::from(self.layers)
            * u64::from(self.kv_heads)
            * u64::from(self.head_dim)
            * 2
            * u64::from(self.dtype_size)
    }
}

/// Default memory factors for common model architectures.
/// Used when a model is not explicitly configured in kv.toml.
pub fn default_memory_factors() -> HashMap<String, ModelMemoryFactor> {
    let mut map = HashMap::new();

    // LLaMA-3 8B: 32 layers, 8 KV heads (GQA), 128 head_dim, fp16
    map.insert(
        "llama3-8b".to_string(),
        ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        },
    );

    // Qwen3-70B: 80 layers, 8 KV heads (GQA), 128 head_dim, fp16
    map.insert(
        "qwen3-70b".to_string(),
        ModelMemoryFactor {
            layers: 80,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        },
    );

    map
}

/// Fallback memory factor when model is unknown. Conservative estimate
/// using typical 7B-class architecture parameters.
pub fn fallback_memory_factor() -> ModelMemoryFactor {
    ModelMemoryFactor {
        layers: 32,
        kv_heads: 8,
        head_dim: 128,
        dtype_size: 2,
    }
}

// ---------------------------------------------------------------------------
// Sequence metadata
// ---------------------------------------------------------------------------

/// Metadata for a single active inference sequence in the KV cache.
///
/// Tracks memory consumption, access patterns, and priority for eviction
/// scoring. Created on allocation, updated on each token generation (via
/// `touch()`), removed on deallocation.
#[derive(Debug, Clone)]
pub struct SequenceMeta {
    /// Unique sequence identifier.
    pub seq_id: SequenceId,
    /// Instance currently hosting this sequence's KV cache.
    pub instance_id: InstanceId,
    /// Backend model name (used to look up memory factors).
    pub model_name: String,
    /// Current context length in tokens.
    pub tokens: usize,
    /// Actual memory consumed in bytes (computed from tokens + model factor).
    pub kv_bytes: u64,
    /// Request priority at allocation time.
    pub priority: Priority,
    /// When this sequence was first allocated.
    pub created_at: Instant,
    /// Last time this sequence was accessed (token generated or touched).
    pub last_access: Instant,
    /// Number of inference requests served by this sequence.
    pub request_count: u32,
    /// Average gap between consecutive requests. Used for reuse prediction.
    /// Zero if request_count < 2.
    pub avg_inter_request_gap: Duration,
    /// Timestamp of the previous request (for computing inter-request gap).
    /// Not part of the public interface; used internally by `record_request()`.
    prev_request_at: Option<Instant>,
    /// Whether this sequence was previously evicted and re-admitted.
    /// The anti-thrashing guard uses this to apply a penalty.
    pub was_readmitted: bool,
    /// Timestamp of the last eviction (if any). Used by the anti-thrashing
    /// guard to detect rapid evict/re-admit cycles.
    pub last_eviction_at: Option<Instant>,
}

impl SequenceMeta {
    /// Create new sequence metadata with initial memory estimate.
    pub fn new(
        seq_id: SequenceId,
        instance_id: InstanceId,
        model_name: String,
        tokens: usize,
        priority: Priority,
        factor: &ModelMemoryFactor,
    ) -> Self {
        let now = Instant::now();
        let kv_bytes = factor.estimate_bytes(tokens);

        Self {
            seq_id,
            instance_id,
            model_name,
            tokens,
            kv_bytes,
            priority,
            created_at: now,
            last_access: now,
            request_count: 1,
            avg_inter_request_gap: Duration::ZERO,
            prev_request_at: None,
            was_readmitted: false,
            last_eviction_at: None,
        }
    }

    /// Update last access time. Called on each token generation.
    pub fn touch(&mut self) {
        self.last_access = Instant::now();
    }

    /// Record a new inference request for this sequence.
    /// Updates request count and rolling average inter-request gap.
    pub fn record_request(&mut self) {
        let now = Instant::now();
        self.last_access = now;
        self.request_count = self.request_count.saturating_add(1);

        if let Some(prev) = self.prev_request_at {
            let gap = now.duration_since(prev);
            // Exponential moving average: new_avg = 0.3 * gap + 0.7 * old_avg
            if self.avg_inter_request_gap == Duration::ZERO {
                self.avg_inter_request_gap = gap;
            } else {
                #[allow(clippy::cast_precision_loss)]
                // Acceptable: nanosecond EMA doesn't need full u128 precision
                let old_nanos = self.avg_inter_request_gap.as_nanos() as f64;
                #[allow(clippy::cast_precision_loss)]
                let new_nanos = gap.as_nanos() as f64;
                let avg_nanos = 0.3 * new_nanos + 0.7 * old_nanos;
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                // avg_nanos is always non-negative (durations are non-negative)
                let avg_nanos_u64 = avg_nanos as u64;
                self.avg_inter_request_gap = Duration::from_nanos(avg_nanos_u64);
            }
        }

        self.prev_request_at = Some(now);
    }

    /// Update token count and recompute memory estimate.
    pub fn update_tokens(&mut self, new_tokens: usize, factor: &ModelMemoryFactor) {
        self.tokens = new_tokens;
        self.kv_bytes = factor.estimate_bytes(new_tokens);
    }

    /// Time since this sequence was last accessed.
    pub fn idle_time(&self) -> Duration {
        self.last_access.elapsed()
    }

    /// Time since this sequence was created.
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Mark this sequence as having been evicted (for re-admission tracking).
    pub fn mark_evicted(&mut self) {
        self.last_eviction_at = Some(Instant::now());
    }

    /// Mark this sequence as re-admitted after a prior eviction.
    pub fn mark_readmitted(&mut self) {
        self.was_readmitted = true;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn test_factor() -> ModelMemoryFactor {
        ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        }
    }

    #[test]
    fn test_memory_estimation_formula() {
        let factor = test_factor();
        // tokens * 32 * 8 * 128 * 2 * 2 = tokens * 131072
        assert_eq!(factor.estimate_bytes(1), 131_072);
        assert_eq!(factor.estimate_bytes(1024), 131_072 * 1024);
        assert_eq!(factor.bytes_per_token(), 131_072);
    }

    #[test]
    fn test_memory_estimation_fp8() {
        let factor = ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 1, // fp8
        };
        // Half the bytes of fp16
        assert_eq!(factor.estimate_bytes(1), 65_536);
    }

    #[test]
    fn test_memory_estimation_gqa_70b() {
        let factor = ModelMemoryFactor {
            layers: 80,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        };
        // 80 * 8 * 128 * 2 * 2 = 327680 per token
        assert_eq!(factor.bytes_per_token(), 327_680);
        // 2048 tokens = ~640 MB
        assert_eq!(factor.estimate_bytes(2048), 327_680 * 2048);
    }

    #[test]
    fn test_sequence_meta_creation() {
        let factor = test_factor();
        let meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        assert_eq!(meta.tokens, 512);
        assert_eq!(meta.kv_bytes, 131_072 * 512);
        assert_eq!(meta.request_count, 1);
        assert_eq!(meta.avg_inter_request_gap, Duration::ZERO);
        assert!(!meta.was_readmitted);
        assert!(meta.last_eviction_at.is_none());
    }

    #[test]
    fn test_touch_updates_last_access() {
        let factor = test_factor();
        let mut meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        let before = meta.last_access;
        thread::sleep(Duration::from_millis(5));
        meta.touch();
        assert!(meta.last_access > before);
    }

    #[test]
    fn test_record_request_updates_gap() {
        let factor = test_factor();
        let mut meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        assert_eq!(meta.request_count, 1);

        // First record_request sets prev_request_at
        meta.record_request();
        assert_eq!(meta.request_count, 2);

        thread::sleep(Duration::from_millis(10));

        // Second record_request computes gap
        meta.record_request();
        assert_eq!(meta.request_count, 3);
        assert!(meta.avg_inter_request_gap > Duration::ZERO);
    }

    #[test]
    fn test_update_tokens() {
        let factor = test_factor();
        let mut meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        assert_eq!(meta.tokens, 512);
        meta.update_tokens(1024, &factor);
        assert_eq!(meta.tokens, 1024);
        assert_eq!(meta.kv_bytes, 131_072 * 1024);
    }

    #[test]
    fn test_idle_time_increases() {
        let factor = test_factor();
        let meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        thread::sleep(Duration::from_millis(5));
        assert!(meta.idle_time() >= Duration::from_millis(4));
    }

    #[test]
    fn test_eviction_markers() {
        let factor = test_factor();
        let mut meta = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );

        assert!(!meta.was_readmitted);
        assert!(meta.last_eviction_at.is_none());

        meta.mark_evicted();
        assert!(meta.last_eviction_at.is_some());

        meta.mark_readmitted();
        assert!(meta.was_readmitted);
    }

    #[test]
    fn test_default_memory_factors() {
        let factors = default_memory_factors();
        assert!(factors.contains_key("llama3-8b"));
        assert!(factors.contains_key("qwen3-70b"));

        let llama = &factors["llama3-8b"];
        assert_eq!(llama.layers, 32);
        assert_eq!(llama.kv_heads, 8);
    }

    #[test]
    fn test_fallback_factor() {
        let factor = fallback_memory_factor();
        assert_eq!(factor.layers, 32);
        assert_eq!(factor.dtype_size, 2);
    }
}
