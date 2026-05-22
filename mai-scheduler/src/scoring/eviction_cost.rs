//! Eviction cost penalty scorer for multi-factor placement.
//!
//! If placing a request on an instance would require evicting cached KV
//! sequences from VRAM, this scorer penalizes the placement proportionally
//! to the value of the evicted sequences. Higher-value sequences (frequently
//! accessed, recently used) are more expensive to evict.
//!
//! # Formula
//!
//! ```text
//! needed_bytes = estimated_kv_bytes(prompt_tokens + max_tokens)
//! if instance has enough free VRAM:
//!   penalty = 0.0
//! else:
//!   candidates = kv_manager.eviction_candidates(needed_bytes)
//!   total_value = sum(1.0 / max(score, 0.01) for each candidate)
//!   penalty = clamp(total_value / max_eviction_cost, 0.0, 1.0)
//! ```
//!
//! The eviction score is the inverse of the sequence's evictability: low
//! eviction scores mean the sequence is valuable (recently used, frequently
//! accessed). Evicting such sequences is expensive.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::kv::manager::KvCacheManager;
use crate::types::{InstanceState, ScheduleRequest};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Eviction cost scoring parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionCostConfig {
    /// Normalization ceiling for eviction cost. Costs at or above this value
    /// produce a normalized penalty of 1.0. Default: 50.0.
    #[serde(default = "default_max_eviction_cost")]
    pub max_eviction_cost: f64,

    /// Rough per-token KV cache byte estimate for sizing checks.
    /// Used when per-model factors are not available. Default: 131072 (128 KB).
    #[serde(default = "default_bytes_per_token")]
    pub default_bytes_per_token: f64,
}

fn default_max_eviction_cost() -> f64 {
    50.0
}

fn default_bytes_per_token() -> f64 {
    131_072.0
}

impl Default for EvictionCostConfig {
    fn default() -> Self {
        Self {
            max_eviction_cost: default_max_eviction_cost(),
            default_bytes_per_token: default_bytes_per_token(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// Compute the eviction cost penalty for placing a request on an instance.
///
/// Returns a value in `[0.0, 1.0]` where 0.0 means no eviction needed and
/// 1.0 means evicting maximally valuable sequences.
///
/// Returns 0.0 if no KV cache manager is configured.
#[allow(clippy::cast_precision_loss)]
pub fn eviction_penalty(
    _state: &InstanceState,
    request: &ScheduleRequest,
    kv_manager: Option<&Arc<dyn KvCacheManager>>,
    config: &EvictionCostConfig,
) -> f64 {
    let kv = match kv_manager {
        Some(k) => k,
        None => return 0.0,
    };

    if config.max_eviction_cost <= 0.0 {
        return 0.0;
    }

    // Estimate how many bytes this request needs
    let estimated_tokens = (request.prompt_tokens + request.max_tokens) as usize;
    let estimated_bytes = (estimated_tokens as f64 * config.default_bytes_per_token) as u64;

    // Check if the request fits without eviction
    let free = kv.free_bytes();
    if estimated_bytes <= free {
        return 0.0;
    }

    // We need to evict. Ask the KV manager for candidates.
    let needed = estimated_bytes.saturating_sub(free);
    let candidates = kv.eviction_candidates(needed);

    if candidates.is_empty() {
        // No candidates available, can't evict, max penalty
        return 1.0;
    }

    // Sum up the "value" of sequences we'd need to evict.
    // Eviction score: higher = more evictable (less valuable).
    // Value = inverse of eviction score: high-value sequences have low scores.
    let mut total_value = 0.0;
    let mut freed_so_far = 0_u64;

    for (_seq_id, bytes_freed, eviction_score) in &candidates {
        if freed_so_far >= needed {
            break;
        }
        // Inverse of eviction score = value. Floor at 0.01 to avoid division by zero.
        let value = 1.0 / eviction_score.max(0.01);
        total_value += value;
        freed_so_far += bytes_freed;
    }

    (total_value / config.max_eviction_cost).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics, InstanceState,
        Priority, ScheduleRequest,
    };

    fn make_state() -> InstanceState {
        InstanceState {
            config: InstanceConfig {
                id: InstanceId::new("test:0"),
                model_name: "test-model".to_string(),
                adapter_type: "test".to_string(),
                gpu_ids: vec![GpuId::new(0)],
                max_batch_size: 16,
                vram_allocated: 16_000_000_000,
                capabilities: InstanceCapabilities::default(),
            },
            metrics: InstanceMetrics::default(),
        }
    }

    fn make_request(prompt_tokens: u32, max_tokens: u32) -> ScheduleRequest {
        ScheduleRequest {
            prompt_tokens,
            max_tokens,
            ..ScheduleRequest::new("test-model", Priority::Normal)
        }
    }

    #[test]
    fn test_no_kv_manager_zero_penalty() {
        let state = make_state();
        let req = make_request(100, 200);
        let config = EvictionCostConfig::default();
        assert!(
            (eviction_penalty(&state, &req, None, &config)).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_plenty_of_room_zero_penalty() {
        use crate::kv::{HeuristicKvCacheManager, KvCacheConfig};

        let kv_config = KvCacheConfig {
            total_budget_bytes: 100_000_000_000, // 100 GB, way more than needed
            ..KvCacheConfig::default()
        };
        let kv: Arc<dyn KvCacheManager> = Arc::new(HeuristicKvCacheManager::new(kv_config));

        let state = make_state();
        let req = make_request(100, 200);
        let config = EvictionCostConfig::default();

        assert!(
            (eviction_penalty(&state, &req, Some(&kv), &config)).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_zero_max_cost_returns_zero() {
        let config = EvictionCostConfig {
            max_eviction_cost: 0.0,
            ..EvictionCostConfig::default()
        };
        let state = make_state();
        let req = make_request(100, 200);
        assert!(
            (eviction_penalty(&state, &req, None, &config)).abs() < f64::EPSILON
        );
    }
}
