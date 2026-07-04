//! Multi-factor scoring engine for placement decisions.
//!
//! This module replaces the Phase 1 least-loaded scorer with a composite
//! scoring function that weighs five factors: latency, memory pressure,
//! topology cost, eviction cost, and batching benefit. Lower composite
//! score = better candidate.
//!
//! # Module Structure
//!
//! - `scorer`: orchestrator that combines sub-scores into a single decision
//! - `latency`: queue-based latency estimation
//! - `memory`: VRAM pressure penalty with configurable exponent
//! - `topology`: GPU interconnect cost for tensor-parallel
//! - `eviction`: eviction cost penalty
//! - `batching`: batch fit benefit
//!
//! # Extension Points
//!
//! Each sub-scorer is a standalone function called by `MultiFactorScorer::score()`.
//! New scoring factors can be added by writing a sub-scorer function and
//! wiring it into the `MultiFactorScorer::score()` method.

pub mod scorer;

// Sub-scorer modules
pub mod batching;
pub mod eviction_cost;
pub mod latency;
pub mod memory;
pub mod topology_score;

// Re-exports
pub use batching::BatchBenefitConfig;
pub use eviction_cost::EvictionCostConfig;
pub use latency::LatencyConfig;
pub use memory::MemoryConfig;
pub use scorer::{
    MultiFactorScorer, ScoreBreakdown, ScoringConfig, build_multi_factor_scorer,
    build_multi_factor_scorer_with_reason, build_scorer,
};
pub use topology_score::TopologyScoreConfig;
