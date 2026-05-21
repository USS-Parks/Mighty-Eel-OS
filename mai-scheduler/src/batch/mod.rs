//! Continuous batching subsystem for the MAI scheduler.
//!
//! This module implements GPU-aware continuous batching: sequences enter and
//! leave the active batch between generation steps rather than waiting for
//! the entire batch to complete.
//!
//! # Components
//!
//! - **BatchBuilder**: per-instance orchestrator that runs `build_step()`
//!   each inference iteration. Manages the active batch and waiting queue.
//!
//! - **AdmissionController**: dual-threshold VRAM admission policy.
//!   Three operating regions: aggressive (<80%), selective (80-90%),
//!   eviction-required (>90%).
//!
//! - **PreemptionPolicy**: emergency-only preemption for active batch
//!   members when VRAM exceeds 95%. Absolute last resort.
//!
//! - **BatchMetrics**: rolling-window metrics (batch size, utilization,
//!   admission rate, wait time percentiles). Thread-safe via atomics.
//!
//! # Usage
//!
//! ```ignore
//! use mai_scheduler::batch::{BatchBuilder, BatchConfig, VramState};
//!
//! let builder = BatchBuilder::new("llama3-8b", BatchConfig::default());
//! // ... enqueue requests, call build_step() each iteration
//! ```

pub mod admission;
pub mod builder;
pub mod metrics;
pub mod preemption;

// Re-exports: the public API surface of the batch subsystem.
pub use builder::{
    ActiveSequence, BatchBuilder, BatchConfig, BatchDecision, QueuedRequest, VramState,
};

// Re-export configs for TOML deserialization
pub use admission::{AdmissionConfig, AdmissionController, AdmissionDecision};
pub use metrics::{BatchMetrics, BatchMetricsSnapshot, MetricsConfig};
pub use preemption::{
    PreemptionCandidate, PreemptionConfig, PreemptionPolicy, PreemptionResult,
};
