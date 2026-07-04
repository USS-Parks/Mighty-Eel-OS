//! MAI Scheduler - Inference request routing and placement
//!
//! This crate is the central decision engine for the MAI. Every inference
//! request flows through the `Scheduler` trait to get a placement decision.
//! No other component makes routing decisions.
//!
//! # Architecture
//!
//! - `Scheduler` trait: object-safe, `&self`, concurrent. Stored as
//!   `Arc<dyn Scheduler>` in AppState.
//! - `DefaultScheduler`: production implementation composing the registry,
//!   placement engine, and alias resolver.
//! - `InstanceRegistry`: DashMap-backed thread-safe instance tracking.
//! - `PlacementEngine`: pluggable scoring function for candidate selection.
//! - `AliasResolver`: maps user-facing model names to backend identifiers.
//! - `GpuTopology`: GPU interconnect graph with NVLink/PCIe edge weights.
//!
//! # Extension Points
//!

#![forbid(unsafe_code)]

pub mod aliases;
pub mod balancer;
pub mod batch;
pub mod decision_cache;
pub mod default;
pub mod kv;
pub mod metrics;
pub mod placement;
pub mod power;
pub mod preemption;
pub mod registry;
pub mod scheduler;
pub mod scoring;
pub mod topology;
pub mod traces;
pub mod types;

// Re-exports for convenience
pub use batch::{BatchBuilder, BatchConfig, BatchDecision};
pub use default::DefaultScheduler;
pub use kv::manager::KvCacheManager;
pub use kv::{HeuristicKvCacheManager, KvCacheConfig};
pub use metrics::{MetricsCollector, MetricsConfig};
pub use power::{PowerControllerConfig, PowerStateController};
pub use scheduler::Scheduler;
pub use scoring::{
    MultiFactorScorer, ScoreBreakdown, ScoringConfig, build_multi_factor_scorer,
    build_multi_factor_scorer_with_reason, build_scorer,
};
pub use topology::{GpuTopology, TopologyConfig, TopologyError};
pub use types::{
    ClusterMetrics, GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics,
    InstanceState, ModelAlias, Priority, ScheduleDecision, ScheduleRequest, SchedulerConfig,
    SchedulerError, ScoringFn, ScoringReasonFn, SequenceId,
};
