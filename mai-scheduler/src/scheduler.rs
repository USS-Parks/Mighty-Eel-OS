//! The Scheduler trait: the single authority for inference request placement.
//!
//! # Design Principles
//!
//! 1. **Object-safe**: The trait uses `&self` with interior mutability so it
//!    can be stored as `Arc<dyn Scheduler>` in AppState. No generics, no
//!    associated types, no `async fn` in the trait.
//!
//! 2. **Concurrent**: Multiple HTTP handler tasks call `schedule()` in
//!    parallel. The implementation must not hold a global write lock during
//!    scoring. DashMap + atomics internally, not `Mutex<Self>`.
//!
//! 3. **Single authority**: No handler, adapter, or other component makes
//!    placement decisions. All inference routing flows through this trait.
//!
//! 4. **Extensible**: topology awareness, KV cache management, and
//!    continuous batching layer on top. The trait surface accommodates
//!    those additions without breaking changes.

use crate::types::{
    ClusterMetrics, GpuId, InstanceConfig, InstanceId, ScheduleDecision, ScheduleRequest,
    SchedulerError, SequenceId,
};

/// The scheduler trait. Implemented by `DefaultScheduler` (this crate) and
/// potentially by test doubles.
///
/// All methods take `&self`. Implementations must provide interior mutability
/// (e.g., via `DashMap`, `RwLock`, atomics) to handle concurrent access from
/// multiple tokio tasks.
///
/// # Object Safety
///
/// This trait is intentionally object-safe so it can be used as
/// `Box<dyn Scheduler>` or `Arc<dyn Scheduler>` in application state.
pub trait Scheduler: Send + Sync {
    /// Place a request on an instance. This is the hot path: called on every
    /// inference request. Must be fast (<1ms) and must not block.
    ///
    /// Returns a `ScheduleDecision` identifying which instance should handle
    /// the request, or an error if no instance is available.
    ///
    /// The implementation:
    /// 1. Resolves the model alias to a backend model name
    /// 2. Finds candidate instances that serve that model
    /// 3. Filters out overloaded or unhealthy instances
    /// 4. Scores candidates and picks the best one
    /// 5. Updates instance metrics (queue_depth, last_sequence_id)
    fn schedule(&self, request: &ScheduleRequest) -> Result<ScheduleDecision, SchedulerError>;

    /// Release a sequence from an instance. Called when an inference request
    /// completes (successfully or with an error). Decrements the instance's
    /// active_sequences and queue_depth counters.
    fn release_sequence(&self, instance: &InstanceId, seq_id: SequenceId);

    /// Register a new model instance with the scheduler. Called when an
    /// adapter starts up or a new model is loaded on an existing adapter.
    fn register_instance(&self, config: InstanceConfig) -> Result<(), SchedulerError>;

    /// Remove an instance from the scheduler. Called when an adapter crashes,
    /// shuts down, or a model is unloaded. In-flight requests to this instance
    /// are not cancelled (they'll fail at the adapter level); the scheduler
    /// simply stops routing new requests to it.
    fn remove_instance(&self, instance: &InstanceId);

    /// Return aggregate metrics across all instances. Used by health endpoints
    /// and operational dashboards. Must not block.
    fn cluster_metrics(&self) -> ClusterMetrics;

    // -----------------------------------------------------------------------
    // Power state integration methods
    // -----------------------------------------------------------------------

    /// Check if a specific instance can be safely demoted. Returns true if the
    /// instance exists and has no active sequences.
    fn can_demote(&self, instance: &InstanceId) -> bool;

    /// Return all GPU IDs that have registered instances.
    fn all_gpu_set(&self) -> Vec<GpuId>;

    /// Return all instance IDs registered on a specific GPU.
    fn instances_on_gpu(&self, gpu_id: GpuId) -> Vec<InstanceId>;

    /// Called when a GPU wakes up so the scheduler can re-register instances
    /// or mark them as healthy. Returns an error if the GPU is unknown.
    fn on_wake_gpu(&self, gpu_id: GpuId) -> Result<(), SchedulerError>;
}
