//! Eviction trigger system for KV cache management.
//!
//! Triggers determine WHEN eviction should occur. The eviction policy
//! (eviction.rs) determines WHAT to evict. Triggers inspect VRAM usage
//! and produce an `EvictionAction` that the manager acts on.
//!
//! Four trigger types:
//!
//! 1. **Threshold**: VRAM usage > 85% (configurable). Evict lowest-priority
//!    sequences until usage drops below threshold.
//!
//! 2. **On-demand**: `can_fit()` returns false. The scheduler wants to place
//!    a new sequence but there's not enough room.
//!
//! 3. **Proactive**: VRAM usage > 75% (configurable). Don't evict yet, but
//!    pre-compute the eviction candidate list so it's ready if needed.
//!
//! 4. **Emergency**: VRAM usage > 95% (configurable). Evict immediately,
//!    bypassing the minimum residency guard.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Trigger thresholds. All values are fractions of total VRAM (0.0 to 1.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Proactive preparation threshold. When VRAM usage exceeds this,
    /// compute eviction candidates in the background.
    /// Default: 0.75 (75%).
    #[serde(default = "default_proactive_threshold")]
    pub proactive_threshold: f64,

    /// Standard eviction threshold. When VRAM usage exceeds this,
    /// start evicting low-priority sequences.
    /// Default: 0.85 (85%).
    #[serde(default = "default_eviction_threshold")]
    pub eviction_threshold: f64,

    /// Emergency eviction threshold. When VRAM usage exceeds this,
    /// evict aggressively, bypassing minimum residency guards.
    /// Default: 0.95 (95%).
    #[serde(default = "default_emergency_threshold")]
    pub emergency_threshold: f64,
}

fn default_proactive_threshold() -> f64 {
    0.75
}
fn default_eviction_threshold() -> f64 {
    0.85
}
fn default_emergency_threshold() -> f64 {
    0.95
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            proactive_threshold: default_proactive_threshold(),
            eviction_threshold: default_eviction_threshold(),
            emergency_threshold: default_emergency_threshold(),
        }
    }
}

// ---------------------------------------------------------------------------
// Trigger evaluation
// ---------------------------------------------------------------------------

/// The type of eviction trigger that fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionTrigger {
    /// VRAM usage exceeded the proactive threshold.
    /// Action: compute candidates but don't evict yet.
    Proactive,
    /// VRAM usage exceeded the standard eviction threshold.
    /// Action: evict sequences respecting all guards.
    Threshold,
    /// `can_fit()` returned false for a new allocation.
    /// Action: evict enough sequences to make room, respecting guards.
    OnDemand,
    /// VRAM usage exceeded the emergency threshold.
    /// Action: evict immediately, bypassing minimum residency guard.
    Emergency,
}

/// The action the manager should take based on trigger evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvictionAction {
    /// VRAM usage is healthy. No action needed.
    None,
    /// Pre-compute eviction candidates (proactive).
    /// The manager should call `eviction_candidates()` and cache the result
    /// but not perform actual eviction.
    PrepareCandidates,
    /// Evict sequences, respecting all anti-thrashing guards.
    /// `needed_bytes` is the minimum bytes to free.
    Evict {
        /// Minimum bytes to free.
        needed_bytes: u64,
        /// Whether to bypass the minimum residency guard (emergency only).
        bypass_residency: bool,
    },
}

/// Evaluate which trigger fires based on current VRAM usage.
///
/// `used_bytes` is the current KV cache memory in use.
/// `total_bytes` is the total VRAM budget for KV caches.
///
/// Returns the action the manager should take.
#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
pub fn evaluate_triggers(
    used_bytes: u64,
    total_bytes: u64,
    config: &TriggerConfig,
) -> EvictionAction {
    if total_bytes == 0 {
        return EvictionAction::None;
    }

    let usage_ratio = used_bytes as f64 / total_bytes as f64;

    if usage_ratio >= config.emergency_threshold {
        // Emergency: evict aggressively, bypass residency guard
        let target_bytes = (total_bytes as f64 * config.eviction_threshold) as u64;
        let needed = used_bytes.saturating_sub(target_bytes);
        EvictionAction::Evict {
            needed_bytes: needed.max(1),
            bypass_residency: true,
        }
    } else if usage_ratio >= config.eviction_threshold {
        // Standard: evict down to below threshold
        let target_bytes = (total_bytes as f64 * config.proactive_threshold) as u64;
        let needed = used_bytes.saturating_sub(target_bytes);
        EvictionAction::Evict {
            needed_bytes: needed.max(1),
            bypass_residency: false,
        }
    } else if usage_ratio >= config.proactive_threshold {
        // Proactive: prepare candidates but don't evict
        EvictionAction::PrepareCandidates
    } else {
        EvictionAction::None
    }
}

/// Evaluate an on-demand trigger when `can_fit()` returns false.
///
/// `needed_bytes` is the estimated memory for the new sequence.
/// Always respects anti-thrashing guards (not emergency mode).
pub fn on_demand_trigger(needed_bytes: u64) -> EvictionAction {
    EvictionAction::Evict {
        needed_bytes,
        bypass_residency: false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> TriggerConfig {
        TriggerConfig::default()
    }

    #[test]
    fn test_healthy_no_action() {
        let config = default_config();
        let total = 10_000_000_000_u64; // 10 GB
        let used = 5_000_000_000_u64; // 50%

        let action = evaluate_triggers(used, total, &config);
        assert_eq!(action, EvictionAction::None);
    }

    #[test]
    fn test_proactive_threshold() {
        let config = default_config();
        let total = 10_000_000_000_u64;
        let used = 7_600_000_000_u64; // 76%

        let action = evaluate_triggers(used, total, &config);
        assert_eq!(action, EvictionAction::PrepareCandidates);
    }

    #[test]
    fn test_eviction_threshold() {
        let config = default_config();
        let total = 10_000_000_000_u64;
        let used = 8_600_000_000_u64; // 86%

        let action = evaluate_triggers(used, total, &config);
        match action {
            EvictionAction::Evict {
                needed_bytes,
                bypass_residency,
            } => {
                assert!(!bypass_residency, "standard eviction should respect guards");
                // Target is 75% = 7.5 GB, need to free 8.6 - 7.5 = 1.1 GB
                assert!(
                    needed_bytes > 0,
                    "should need to free some bytes: {needed_bytes}"
                );
            }
            other => panic!("expected Evict, got {other:?}"),
        }
    }

    #[test]
    fn test_emergency_threshold() {
        let config = default_config();
        let total = 10_000_000_000_u64;
        let used = 9_600_000_000_u64; // 96%

        let action = evaluate_triggers(used, total, &config);
        match action {
            EvictionAction::Evict {
                bypass_residency, ..
            } => {
                assert!(bypass_residency, "emergency should bypass residency guard");
            }
            other => panic!("expected emergency Evict, got {other:?}"),
        }
    }

    #[test]
    fn test_on_demand_trigger() {
        let action = on_demand_trigger(500_000_000);
        match action {
            EvictionAction::Evict {
                needed_bytes,
                bypass_residency,
            } => {
                assert_eq!(needed_bytes, 500_000_000);
                assert!(!bypass_residency);
            }
            other => panic!("expected Evict, got {other:?}"),
        }
    }

    #[test]
    fn test_zero_total_no_action() {
        let config = default_config();
        let action = evaluate_triggers(100, 0, &config);
        assert_eq!(action, EvictionAction::None);
    }

    #[test]
    fn test_exact_threshold_boundary() {
        let config = default_config();
        let total = 100_u64;

        // Exactly at proactive threshold (75%)
        let action = evaluate_triggers(75, total, &config);
        assert_eq!(action, EvictionAction::PrepareCandidates);

        // Exactly at eviction threshold (85%)
        let action = evaluate_triggers(85, total, &config);
        match action {
            EvictionAction::Evict {
                bypass_residency, ..
            } => {
                assert!(!bypass_residency);
            }
            other => panic!("expected Evict at 85%, got {other:?}"),
        }

        // Exactly at emergency threshold (95%)
        let action = evaluate_triggers(95, total, &config);
        match action {
            EvictionAction::Evict {
                bypass_residency, ..
            } => {
                assert!(bypass_residency);
            }
            other => panic!("expected emergency Evict at 95%, got {other:?}"),
        }
    }

    #[test]
    fn test_custom_thresholds() {
        let config = TriggerConfig {
            proactive_threshold: 0.50,
            eviction_threshold: 0.60,
            emergency_threshold: 0.80,
        };
        let total = 100_u64;

        assert_eq!(evaluate_triggers(40, total, &config), EvictionAction::None);
        assert_eq!(
            evaluate_triggers(55, total, &config),
            EvictionAction::PrepareCandidates
        );
        match evaluate_triggers(65, total, &config) {
            EvictionAction::Evict {
                bypass_residency, ..
            } => assert!(!bypass_residency),
            other => panic!("expected standard Evict, got {other:?}"),
        }
        match evaluate_triggers(85, total, &config) {
            EvictionAction::Evict {
                bypass_residency, ..
            } => assert!(bypass_residency),
            other => panic!("expected emergency Evict, got {other:?}"),
        }
    }
}
