//! Two-tier KV cache controller.
//!
//! Hot tier — sequences in GPU VRAM, actively serving requests.
//! Warm tier — sequences offloaded to CPU pinned memory (via `OffloadManager`).
//! Cold     — sequences fully evicted, requires a re-prefill on next request.
//!
//! This module decides *when* a sequence should move between tiers, based on
//! idle time. It does not perform byte movement; it returns a plan that the
//! caller (the scheduler) applies via the KV manager and offload manager.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::kv::sequence::SequenceMeta;
use crate::types::SequenceId;

use super::offload::SoftEvictionState;

/// Logical tier a sequence currently occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// GPU VRAM, active serving.
    Hot,
    /// CPU pinned memory (offloaded).
    Warm,
    /// Fully evicted.
    Cold,
}

/// Recommended action for a sequence at the time of evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierAction {
    /// Demote a hot sequence to warm tier (i.e. offload to CPU).
    DemoteToWarm(SequenceId),
    /// Promote a warm sequence back to hot tier (i.e. restore to GPU).
    PromoteToHot(SequenceId),
    /// Evict a warm sequence entirely (its CPU footprint can be reclaimed).
    EvictFromWarm(SequenceId),
}

/// Configuration thresholds for the tier controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredCacheConfig {
    /// Idle time above which a hot sequence is eligible for demotion.
    #[serde(default = "default_warm_idle")]
    pub demote_after: Duration,
    /// Idle time above which a warm sequence is evicted from CPU.
    #[serde(default = "default_cold_idle")]
    pub evict_after: Duration,
}

fn default_warm_idle() -> Duration {
    Duration::from_secs(30)
}

fn default_cold_idle() -> Duration {
    Duration::from_secs(300)
}

impl Default for TieredCacheConfig {
    fn default() -> Self {
        Self {
            demote_after: default_warm_idle(),
            evict_after: default_cold_idle(),
        }
    }
}

/// View of one sequence presented to the controller.
///
/// This decouples the controller from the storage of `SequenceMeta` so tests
/// can supply fixed inputs without depending on `Instant::now()`.
#[derive(Debug, Clone, Copy)]
pub struct SequenceObservation {
    /// Sequence identifier.
    pub seq_id: SequenceId,
    /// Idle time since last access.
    pub idle: Duration,
    /// Current eviction state.
    pub state: SoftEvictionState,
}

impl SequenceObservation {
    /// Build an observation from live sequence metadata + an offload state.
    pub fn from_meta(meta: &SequenceMeta, state: SoftEvictionState) -> Self {
        Self {
            seq_id: meta.seq_id,
            idle: meta.idle_time(),
            state,
        }
    }
}

/// Stateless controller that proposes tier transitions.
#[derive(Debug, Clone)]
pub struct TieredCacheController {
    config: TieredCacheConfig,
}

impl TieredCacheController {
    /// New controller with the given thresholds.
    pub fn new(config: TieredCacheConfig) -> Self {
        Self { config }
    }

    /// Default controller with 30s warm / 5min cold thresholds.
    pub fn with_defaults() -> Self {
        Self::new(TieredCacheConfig::default())
    }

    /// Map an observation to its current logical tier.
    pub fn tier_of(obs: SequenceObservation) -> Tier {
        match obs.state {
            SoftEvictionState::Active => Tier::Hot,
            // In-flight transitions count as their destination tier so the
            // controller does not propose a second action against them.
            SoftEvictionState::Offloading | SoftEvictionState::Offloaded => Tier::Warm,
            SoftEvictionState::Restoring => Tier::Hot,
        }
    }

    /// Inspect a slice of observations and return the actions the caller
    /// should apply. Active sequences with no requests in the recent window
    /// are demoted; warm sequences past the cold threshold are evicted.
    pub fn evaluate(&self, observations: &[SequenceObservation]) -> Vec<TierAction> {
        let mut actions = Vec::new();
        for obs in observations {
            match obs.state {
                SoftEvictionState::Active if obs.idle >= self.config.demote_after => {
                    actions.push(TierAction::DemoteToWarm(obs.seq_id));
                }
                SoftEvictionState::Offloaded if obs.idle >= self.config.evict_after => {
                    actions.push(TierAction::EvictFromWarm(obs.seq_id));
                }
                _ => {}
            }
        }
        actions
    }

    /// Propose a promotion when a sequence is needed for an incoming request.
    /// Returns `None` when the sequence is already hot or unknown.
    pub fn plan_promotion(obs: SequenceObservation) -> Option<TierAction> {
        match obs.state {
            SoftEvictionState::Offloaded => Some(TierAction::PromoteToHot(obs.seq_id)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(state: SoftEvictionState, idle_secs: u64) -> SequenceObservation {
        SequenceObservation {
            seq_id: SequenceId::new(),
            idle: Duration::from_secs(idle_secs),
            state,
        }
    }

    #[test]
    fn test_active_sequence_below_threshold_stays_hot() {
        let ctrl = TieredCacheController::with_defaults();
        let actions = ctrl.evaluate(&[obs(SoftEvictionState::Active, 5)]);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_active_sequence_past_demote_threshold_demotes() {
        let ctrl = TieredCacheController::with_defaults();
        let actions = ctrl.evaluate(&[obs(SoftEvictionState::Active, 45)]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TierAction::DemoteToWarm(_)));
    }

    #[test]
    fn test_warm_sequence_below_evict_stays_warm() {
        let ctrl = TieredCacheController::with_defaults();
        let actions = ctrl.evaluate(&[obs(SoftEvictionState::Offloaded, 120)]);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_warm_sequence_past_evict_threshold_is_evicted() {
        let ctrl = TieredCacheController::with_defaults();
        let actions = ctrl.evaluate(&[obs(SoftEvictionState::Offloaded, 600)]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], TierAction::EvictFromWarm(_)));
    }

    #[test]
    fn test_offloading_in_flight_is_left_alone() {
        let ctrl = TieredCacheController::with_defaults();
        let actions = ctrl.evaluate(&[obs(SoftEvictionState::Offloading, 1_000)]);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_plan_promotion_for_offloaded() {
        let observation = obs(SoftEvictionState::Offloaded, 10);
        let action = TieredCacheController::plan_promotion(observation);
        assert!(matches!(action, Some(TierAction::PromoteToHot(_))));
    }

    #[test]
    fn test_plan_promotion_skips_active() {
        let observation = obs(SoftEvictionState::Active, 10);
        assert!(TieredCacheController::plan_promotion(observation).is_none());
    }

    #[test]
    fn test_tier_of_active_is_hot() {
        assert_eq!(
            TieredCacheController::tier_of(obs(SoftEvictionState::Active, 0)),
            Tier::Hot,
        );
        assert_eq!(
            TieredCacheController::tier_of(obs(SoftEvictionState::Offloaded, 0)),
            Tier::Warm,
        );
    }
}
