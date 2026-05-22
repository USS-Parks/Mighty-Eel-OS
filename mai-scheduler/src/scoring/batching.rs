//! Batch fit benefit scorer for multi-factor placement.
//!
//! Rewards instances where a new request can efficiently join an active
//! decode batch. This benefit is subtracted from the total score, making
//! batch-friendly placements more attractive.
//!
//! # Scoring Factors
//!
//! 1. **Batch headroom**: How much room the batch has. An empty batch that
//!    can absorb new work scores higher than a nearly-full one.
//! 2. **Admission region**: Based on VRAM pressure. Aggressive region (low
//!    VRAM usage) scores highest; eviction-required region scores zero.
//! 3. **Queue depth bonus**: Fewer items in the waiting queue means faster
//!    admission, which increases the benefit.
//!
//! # Formula
//!
//! ```text
//! headroom = 1.0 - batch_utilization
//! admission_factor = match vram_region {
//!     Aggressive  => 1.0,
//!     Selective   => 0.5,
//!     Eviction    => 0.0,
//! }
//! queue_factor = 1.0 - clamp(batch_waiting / max_queue, 0.0, 1.0)
//! benefit = headroom * admission_factor * queue_factor
//! ```

use serde::{Deserialize, Serialize};

use crate::types::InstanceState;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Batch benefit scoring parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchBenefitConfig {
    /// VRAM usage fraction below which admission is aggressive. Matches
    /// the admission controller's aggressive_threshold. Default: 0.80.
    #[serde(default = "default_aggressive_threshold")]
    pub aggressive_threshold: f64,

    /// VRAM usage fraction above which admission requires eviction. Matches
    /// the admission controller's eviction_threshold. Default: 0.90.
    #[serde(default = "default_eviction_threshold")]
    pub eviction_threshold: f64,

    /// Maximum waiting queue depth for normalization. Default: 128.
    #[serde(default = "default_max_queue")]
    pub max_queue_depth: u32,
}

fn default_aggressive_threshold() -> f64 {
    0.80
}

fn default_eviction_threshold() -> f64 {
    0.90
}

fn default_max_queue() -> u32 {
    128
}

impl Default for BatchBenefitConfig {
    fn default() -> Self {
        Self {
            aggressive_threshold: default_aggressive_threshold(),
            eviction_threshold: default_eviction_threshold(),
            max_queue_depth: default_max_queue(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// Compute the batching benefit for an instance.
///
/// Returns a value in `[0.0, 1.0]` where 0.0 means no batching benefit
/// (batch full, high VRAM pressure, or long queue) and 1.0 means ideal
/// batching conditions (empty batch, low VRAM, no queue).
///
/// This value is SUBTRACTED from the total score, so higher benefit = lower
/// total score = better candidate.
#[allow(clippy::cast_precision_loss)]
pub fn batching_benefit(state: &InstanceState, config: &BatchBenefitConfig) -> f64 {
    // Headroom: how much room the batch has (1.0 = empty, 0.0 = full)
    let headroom = 1.0 - state.metrics.batch_utilization.clamp(0.0, 1.0);

    // Admission region based on VRAM usage ratio
    let vram_ratio = if state.config.vram_allocated == 0 {
        1.0
    } else {
        state.metrics.vram_used as f64 / state.config.vram_allocated as f64
    };

    let admission_factor = if vram_ratio < config.aggressive_threshold {
        1.0
    } else if vram_ratio < config.eviction_threshold {
        0.5
    } else {
        0.0
    };

    // Queue factor: fewer waiting items = faster admission = more benefit
    let queue_ratio = if config.max_queue_depth == 0 {
        1.0
    } else {
        f64::from(state.metrics.batch_waiting_count)
            / f64::from(config.max_queue_depth)
    };
    let queue_factor = 1.0 - queue_ratio.clamp(0.0, 1.0);

    (headroom * admission_factor * queue_factor).clamp(0.0, 1.0)
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

    fn make_state(
        batch_utilization: f64,
        vram_used: u64,
        vram_allocated: u64,
        batch_waiting: u32,
    ) -> InstanceState {
        InstanceState {
            config: InstanceConfig {
                id: InstanceId::new("test:0"),
                model_name: "test-model".to_string(),
                adapter_type: "test".to_string(),
                gpu_ids: vec![GpuId::new(0)],
                max_batch_size: 16,
                vram_allocated,
                capabilities: InstanceCapabilities::default(),
            },
            metrics: InstanceMetrics {
                batch_utilization,
                vram_used,
                batch_waiting_count: batch_waiting,
                ..InstanceMetrics::default()
            },
        }
    }

    #[test]
    fn test_ideal_conditions_max_benefit() {
        let config = BatchBenefitConfig::default();
        // Empty batch, low VRAM, no queue
        let state = make_state(0.0, 0, 16_000_000_000, 0);
        let benefit = batching_benefit(&state, &config);
        assert!(
            (benefit - 1.0).abs() < f64::EPSILON,
            "ideal conditions should give max benefit, got {benefit}"
        );
    }

    #[test]
    fn test_full_batch_zero_benefit() {
        let config = BatchBenefitConfig::default();
        let state = make_state(1.0, 0, 16_000_000_000, 0);
        let benefit = batching_benefit(&state, &config);
        assert!(
            benefit.abs() < f64::EPSILON,
            "full batch should give zero benefit, got {benefit}"
        );
    }

    #[test]
    fn test_high_vram_zero_benefit() {
        let config = BatchBenefitConfig::default();
        // VRAM above eviction threshold (90%)
        let state = make_state(0.0, 15_000_000_000, 16_000_000_000, 0);
        let benefit = batching_benefit(&state, &config);
        assert!(
            benefit.abs() < f64::EPSILON,
            "high VRAM should give zero benefit, got {benefit}"
        );
    }

    #[test]
    fn test_selective_region_half_factor() {
        let config = BatchBenefitConfig::default();
        // VRAM at 85% (between 80% and 90% = selective region)
        let state = make_state(0.0, 13_600_000_000, 16_000_000_000, 0);
        let benefit = batching_benefit(&state, &config);
        assert!(
            (benefit - 0.5).abs() < 0.01,
            "selective region should give ~0.5 benefit, got {benefit}"
        );
    }

    #[test]
    fn test_queue_reduces_benefit() {
        let config = BatchBenefitConfig::default(); // max_queue=128
        let no_queue = batching_benefit(
            &make_state(0.0, 0, 16_000_000_000, 0),
            &config,
        );
        let with_queue = batching_benefit(
            &make_state(0.0, 0, 16_000_000_000, 64),
            &config,
        );
        assert!(
            no_queue > with_queue,
            "queue should reduce benefit"
        );
    }

    #[test]
    fn test_full_queue_zero_benefit() {
        let config = BatchBenefitConfig::default(); // max_queue=128
        let state = make_state(0.0, 0, 16_000_000_000, 128);
        let benefit = batching_benefit(&state, &config);
        assert!(
            benefit.abs() < f64::EPSILON,
            "full queue should give zero benefit, got {benefit}"
        );
    }

    #[test]
    fn test_partial_batch_partial_benefit() {
        let config = BatchBenefitConfig::default();
        // 50% batch utilization, low VRAM, no queue
        let state = make_state(0.5, 0, 16_000_000_000, 0);
        let benefit = batching_benefit(&state, &config);
        assert!(
            (benefit - 0.5).abs() < 0.01,
            "50% batch should give ~0.5 benefit, got {benefit}"
        );
    }
}
