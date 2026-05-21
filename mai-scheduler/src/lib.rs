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
//! - Session 16: topology discovery, weighted graph, placement penalty (done).
//! - Session 17: KV cache manager integrated for cache-aware placement.
//! - Session 18: continuous batching engine with admission control (done).
//! - Session 19: multi-factor scorer replaces least-loaded default.
//! - Session 20: admission control with request queuing.
//! - Session 21: autoscaler for dynamic instance count.

#![forbid(unsafe_code)]

pub mod aliases;
pub mod batch;
pub mod default;
pub mod kv;
pub mod placement;
pub mod registry;
pub mod scheduler;
pub mod topology;
pub mod types;

// Re-exports for convenience
pub use batch::{BatchBuilder, BatchConfig, BatchDecision};
pub use default::DefaultScheduler;
pub use kv::manager::KvCacheManager;
pub use kv::{HeuristicKvCacheManager, KvCacheConfig};
pub use scheduler::Scheduler;
pub use topology::{GpuTopology, TopologyConfig, TopologyError};
pub use types::{
    ClusterMetrics, GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics,
    InstanceState, ModelAlias, Priority, ScheduleDecision, ScheduleRequest, SchedulerConfig,
    SchedulerError, ScoringFn, SequenceId,
};
