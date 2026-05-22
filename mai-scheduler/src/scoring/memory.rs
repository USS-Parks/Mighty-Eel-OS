//! Memory pressure penalty scorer for multi-factor placement.
//!
//! Computes a penalty based on VRAM utilization with a configurable exponent.
//! The exponential curve makes the penalty increase sharply near capacity,
//! discouraging "just barely fits" placements that cause eviction cascades.
//!
//! # Formula
//!
//! ```text
//! usage_ratio = vram_used / vram_allocated
//! penalty = clamp(usage_ratio ^ pressure_exponent, 0.0, 1.0)
//! ```
//!
//! With the default exponent of 2.0 (quadratic):
//! - 50% usage -> 0.25 penalty
//! - 75% usage -> 0.5625 penalty
//! - 90% usage -> 0.81 penalty
//! - 95% usage -> 0.9025 penalty
//!
//! Higher exponents make the curve steeper, punishing high utilization harder.

use serde::{Deserialize, Serialize};

use crate::types::InstanceState;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Memory pressure scoring parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Exponent for the pressure curve. Higher values make penalty grow
    /// faster near capacity. Default: 2.0 (quadratic).
    #[serde(default = "default_pressure_exponent")]
    pub pressure_exponent: f64,
}

fn default_pressure_exponent() -> f64 {
    2.0
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            pressure_exponent: default_pressure_exponent(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// Compute the memory pressure penalty for an instance.
///
/// Returns a value in `[0.0, 1.0]` where 0.0 means empty VRAM and 1.0
/// means at or above capacity.
#[allow(clippy::cast_precision_loss)]
pub fn memory_penalty(state: &InstanceState, config: &MemoryConfig) -> f64 {
    if state.config.vram_allocated == 0 {
        return 1.0; // No VRAM allocated = treat as fully pressured
    }

    let usage_ratio =
        state.metrics.vram_used as f64 / state.config.vram_allocated as f64;
    let clamped_ratio = usage_ratio.clamp(0.0, 1.0);

    clamped_ratio.powf(config.pressure_exponent).clamp(0.0, 1.0)
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

    fn make_state(vram_used: u64, vram_allocated: u64) -> InstanceState {
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
                vram_used,
                ..InstanceMetrics::default()
            },
        }
    }

    #[test]
    fn test_empty_vram_zero_penalty() {
        let state = make_state(0, 16_000_000_000);
        let config = MemoryConfig::default();
        assert!((memory_penalty(&state, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_full_vram_max_penalty() {
        let state = make_state(16_000_000_000, 16_000_000_000);
        let config = MemoryConfig::default();
        assert!((memory_penalty(&state, &config) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_quadratic_curve() {
        let config = MemoryConfig {
            pressure_exponent: 2.0,
        };
        // 50% usage -> 0.25
        let state = make_state(8_000_000_000, 16_000_000_000);
        let penalty = memory_penalty(&state, &config);
        assert!(
            (penalty - 0.25).abs() < 0.001,
            "50% usage with exp=2 should be ~0.25, got {penalty}"
        );
    }

    #[test]
    fn test_cubic_curve_steeper() {
        let config = MemoryConfig {
            pressure_exponent: 3.0,
        };
        // 50% usage -> 0.125 (steeper at low usage, harsher at high)
        let state = make_state(8_000_000_000, 16_000_000_000);
        let penalty = memory_penalty(&state, &config);
        assert!(
            (penalty - 0.125).abs() < 0.001,
            "50% usage with exp=3 should be ~0.125, got {penalty}"
        );
    }

    #[test]
    fn test_linear_curve() {
        let config = MemoryConfig {
            pressure_exponent: 1.0,
        };
        // 75% usage -> 0.75
        let state = make_state(12_000_000_000, 16_000_000_000);
        let penalty = memory_penalty(&state, &config);
        assert!(
            (penalty - 0.75).abs() < 0.001,
            "75% usage with exp=1 should be ~0.75, got {penalty}"
        );
    }

    #[test]
    fn test_zero_allocated_returns_max() {
        let state = make_state(0, 0);
        let config = MemoryConfig::default();
        assert!((memory_penalty(&state, &config) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_higher_usage_higher_penalty() {
        let config = MemoryConfig::default();
        let low = memory_penalty(&make_state(4_000_000_000, 16_000_000_000), &config);
        let high = memory_penalty(&make_state(12_000_000_000, 16_000_000_000), &config);
        assert!(
            high > low,
            "higher VRAM usage should produce higher penalty"
        );
    }
}
