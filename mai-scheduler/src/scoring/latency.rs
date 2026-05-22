//! Latency penalty scorer for multi-factor placement.
//!
//! Estimates the wait time a request will experience at a candidate instance
//! based on current queue depth and batch state. The penalty is normalized
//! against a configurable target latency.
//!
//! # Formula
//!
//! ```text
//! queue_wait = (queue_depth + batch_waiting) * avg_step_time_ms
//! batch_drain = active_batch_remaining_tokens * per_token_time_ms
//! raw_latency = queue_wait + batch_drain
//! penalty = clamp(raw_latency / target_latency_ms, 0.0, 1.0)
//! ```
//!
//! A penalty of 1.0 means the estimated wait equals or exceeds the target.
//! The weight applied to this penalty is configured in `ScoringConfig`.

use serde::{Deserialize, Serialize};

use crate::types::InstanceState;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Latency estimation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyConfig {
    /// Target latency in milliseconds. Estimated latencies at or above this
    /// value produce a normalized penalty of 1.0.
    #[serde(default = "default_target_latency_ms")]
    pub target_latency_ms: f64,

    /// Estimated time per inference step in milliseconds.
    /// Used to convert queue depth to estimated wait time.
    #[serde(default = "default_avg_step_time_ms")]
    pub avg_step_time_ms: f64,

    /// Estimated time per generated token in milliseconds.
    /// Used to estimate how long the current batch will take to drain.
    #[serde(default = "default_per_token_time_ms")]
    pub per_token_time_ms: f64,
}

fn default_target_latency_ms() -> f64 {
    500.0
}

fn default_avg_step_time_ms() -> f64 {
    20.0
}

fn default_per_token_time_ms() -> f64 {
    5.0
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            target_latency_ms: default_target_latency_ms(),
            avg_step_time_ms: default_avg_step_time_ms(),
            per_token_time_ms: default_per_token_time_ms(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// Compute the latency penalty for an instance.
///
/// Returns a value in `[0.0, 1.0]` where 0.0 means no wait and 1.0 means
/// the estimated wait meets or exceeds the target latency.
#[allow(clippy::cast_precision_loss)]
pub fn latency_penalty(state: &InstanceState, config: &LatencyConfig) -> f64 {
    if config.target_latency_ms <= 0.0 {
        return 0.0;
    }

    // Queue wait: each queued + batch-waiting request adds avg_step_time
    let queue_items = f64::from(state.metrics.queue_depth)
        + f64::from(state.metrics.batch_waiting_count);
    let queue_wait = queue_items * config.avg_step_time_ms;

    // Batch drain: estimate how long the active batch's remaining work takes.
    // We approximate remaining tokens from decode_slots_used (each slot has
    // work proportional to remaining generation). Without per-sequence token
    // counts at this level, we use decode_slots_used as a proxy.
    let batch_drain = f64::from(state.metrics.decode_slots_used) * config.per_token_time_ms;

    let raw_latency = queue_wait + batch_drain;
    (raw_latency / config.target_latency_ms).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics, InstanceState,
    };

    fn make_state(queue_depth: u32, batch_waiting: u32, decode_slots: u32) -> InstanceState {
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
            metrics: InstanceMetrics {
                queue_depth,
                batch_waiting_count: batch_waiting,
                decode_slots_used: decode_slots,
                ..InstanceMetrics::default()
            },
        }
    }

    #[test]
    fn test_empty_instance_zero_penalty() {
        let state = make_state(0, 0, 0);
        let config = LatencyConfig::default();
        assert!((latency_penalty(&state, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_queue_depth_increases_penalty() {
        let config = LatencyConfig::default(); // target=500ms, step=20ms
        let low = latency_penalty(&make_state(2, 0, 0), &config);
        let high = latency_penalty(&make_state(10, 0, 0), &config);
        assert!(high > low, "higher queue depth should produce higher penalty");
    }

    #[test]
    fn test_batch_waiting_adds_to_penalty() {
        let config = LatencyConfig::default();
        let without = latency_penalty(&make_state(5, 0, 0), &config);
        let with = latency_penalty(&make_state(5, 3, 0), &config);
        assert!(
            with > without,
            "batch_waiting_count should increase penalty"
        );
    }

    #[test]
    fn test_decode_slots_contribute() {
        let config = LatencyConfig::default();
        let without = latency_penalty(&make_state(0, 0, 0), &config);
        let with = latency_penalty(&make_state(0, 0, 8), &config);
        assert!(with > without, "decode_slots should increase penalty");
    }

    #[test]
    fn test_clamped_at_one() {
        let config = LatencyConfig {
            target_latency_ms: 100.0,
            avg_step_time_ms: 50.0,
            per_token_time_ms: 10.0,
        };
        // 50 queue items * 50ms = 2500ms >> 100ms target
        let penalty = latency_penalty(&make_state(50, 0, 0), &config);
        assert!(
            (penalty - 1.0).abs() < f64::EPSILON,
            "penalty should clamp at 1.0"
        );
    }

    #[test]
    fn test_zero_target_returns_zero() {
        let config = LatencyConfig {
            target_latency_ms: 0.0,
            ..LatencyConfig::default()
        };
        assert!((latency_penalty(&make_state(10, 5, 8), &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_specific_calculation() {
        let config = LatencyConfig {
            target_latency_ms: 500.0,
            avg_step_time_ms: 20.0,
            per_token_time_ms: 5.0,
        };
        // queue_wait = (5 + 2) * 20 = 140
        // batch_drain = 4 * 5 = 20
        // raw = 160, penalty = 160/500 = 0.32
        let penalty = latency_penalty(&make_state(5, 2, 4), &config);
        assert!(
            (penalty - 0.32).abs() < 0.001,
            "expected ~0.32, got {penalty}"
        );
    }
}
