//! Core type definitions for the MAI scheduler.
//!
//! These types form the public API surface of the scheduler crate. All
//! interactions with the scheduler go through these types. The scheduler
//! trait, instance registry, placement engine, and alias resolver all
//! consume and produce values defined here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Newtypes: strong typing for IDs to prevent accidental swaps
// ---------------------------------------------------------------------------

/// Unique identifier for a model instance (an adapter serving a specific model).
/// Format convention: "adapter_type:index" (e.g., "ollama:0", "vllm:1").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstanceId(pub String);

impl InstanceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for InstanceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for InstanceId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Unique identifier for an inference sequence (conversation session).
/// Used for KV cache locality: repeated requests for the same sequence
/// prefer the instance that last served it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SequenceId(pub Uuid);

impl SequenceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for SequenceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SequenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// GPU identifier. Wraps a u32 ordinal index within the local system.
/// Matches the NVML device index or AMD ROCm ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GpuId(pub u32);

impl GpuId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Display for GpuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gpu:{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Request priority
// ---------------------------------------------------------------------------

/// Request priority levels. Lower numeric value = higher priority.
/// The scheduler uses this for backpressure decisions and queue ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// System-internal requests (health probes, internal bookkeeping).
    /// Never rejected by backpressure.
    System = 0,
    /// Elevated priority (parent profiles, time-sensitive tasks).
    High = 1,
    /// Standard user requests.
    Normal = 2,
    /// Background tasks (batch processing, prefetch, warmup).
    Background = 3,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::High => write!(f, "high"),
            Self::Normal => write!(f, "normal"),
            Self::Background => write!(f, "background"),
        }
    }
}

// ---------------------------------------------------------------------------
// Schedule request (input to scheduler)
// ---------------------------------------------------------------------------

/// A scheduling request. This is what callers (HTTP/gRPC handlers) submit
/// to the scheduler to get a placement decision.
#[derive(Debug, Clone)]
pub struct ScheduleRequest {
    /// Conversation/session identifier. Used for KV cache locality.
    pub session_id: SequenceId,
    /// User-facing model name (alias). Resolved by the alias subsystem.
    pub model_alias: String,
    /// Estimated prompt token count (for capacity estimation).
    pub prompt_tokens: u32,
    /// Maximum tokens to generate (for capacity reservation).
    pub max_tokens: u32,
    /// Request priority.
    pub priority: Priority,
    /// If this is a continuation of a prior sequence, the scheduler prefers
    /// the instance that last served it (KV cache locality hint). Even before
    /// the KV cache manager exists, we track this for routing.
    pub continuation_of: Option<SequenceId>,
    /// Caller-supplied metadata for debugging and telemetry. Never affects
    /// placement decisions; purely observational.
    pub request_metadata: HashMap<String, String>,
}

impl ScheduleRequest {
    /// Create a minimal schedule request for testing or simple cases.
    pub fn new(model_alias: impl Into<String>, priority: Priority) -> Self {
        Self {
            session_id: SequenceId::new(),
            model_alias: model_alias.into(),
            prompt_tokens: 0,
            max_tokens: 2048,
            priority,
            continuation_of: None,
            request_metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Schedule decision (output from scheduler)
// ---------------------------------------------------------------------------

/// The scheduler's placement decision. Tells the caller which instance to
/// route the request to and why.
#[derive(Debug, Clone)]
pub struct ScheduleDecision {
    /// Which instance should handle this request.
    pub instance_id: InstanceId,
    /// GPU(s) assigned for this request (may be empty for CPU-only instances).
    pub assigned_gpus: Vec<GpuId>,
    /// Estimated latency for this placement in milliseconds.
    /// Based on current queue depth and historical throughput.
    pub estimated_latency_ms: u64,
    /// Human-readable placement reason for debugging/telemetry.
    /// Examples: "least-loaded", "continuation-affinity", "only-candidate".
    pub placement_reason: String,
}

// ---------------------------------------------------------------------------
// Instance configuration and state
// ---------------------------------------------------------------------------

/// Instance capabilities bitmap. Each flag indicates a feature the instance
/// supports. Used by placement to filter candidates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceCapabilities {
    /// Maximum context window size in tokens.
    pub context_window: u32,
    /// Whether the instance supports streaming token output.
    pub supports_streaming: bool,
    /// Whether the instance supports batch inference.
    pub supports_batch: bool,
    /// Whether the instance supports embedding generation.
    pub supports_embeddings: bool,
    /// Whether the instance supports structured output (JSON schema).
    pub supports_structured: bool,
    /// Whether the instance supports function/tool calling.
    pub supports_function_calling: bool,
}

/// Static configuration for a model instance. Set at registration time
/// and does not change during the instance's lifetime (unless re-registered).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfig {
    /// Unique instance identifier.
    pub id: InstanceId,
    /// The backend model name this instance serves (e.g., "llama3-8b").
    pub model_name: String,
    /// Adapter type (e.g., "ollama", "vllm", "llamacpp").
    pub adapter_type: String,
    /// GPU(s) assigned to this instance.
    pub gpu_ids: Vec<GpuId>,
    /// Maximum concurrent sequences (batch size limit).
    pub max_batch_size: u32,
    /// VRAM allocated to this instance in bytes.
    pub vram_allocated: u64,
    /// Instance capabilities.
    pub capabilities: InstanceCapabilities,
}

/// Live metrics for an instance. Updated on every request/completion.
/// Read by the placement engine for scoring.
#[derive(Debug, Clone, Default)]
pub struct InstanceMetrics {
    /// Current number of sequences in the instance's queue.
    pub queue_depth: u32,
    /// Number of actively running sequences.
    pub active_sequences: u32,
    /// Current VRAM used in bytes.
    pub vram_used: u64,
    /// Timestamp of the last routed request (epoch millis).
    pub last_request_epoch_ms: u64,
    /// Last sequence ID served (for continuation affinity).
    pub last_sequence_id: Option<SequenceId>,
    // Batch metrics ---
    /// Current batch size (sequences in active forward pass).
    pub batch_size: u32,
    /// Number of sequences waiting in the prefill queue.
    pub prefill_queue_depth: u32,
    /// Number of decode slots currently occupied.
    pub decode_slots_used: u32,
    /// Batch utilization ratio: actual_batch_size / max_batch_size (0.0..1.0).
    pub batch_utilization: f64,
    /// Number of sequences waiting for batch admission.
    pub batch_waiting_count: u32,
}

/// Combined config + live metrics for an instance. This is what the
/// registry stores and what placement reads.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub config: InstanceConfig,
    pub metrics: InstanceMetrics,
}

// ---------------------------------------------------------------------------
// Cluster-level metrics
// ---------------------------------------------------------------------------

/// Aggregate metrics across all instances. Returned by `Scheduler::cluster_metrics()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterMetrics {
    /// Total registered instances.
    pub total_instances: u32,
    /// Instances currently accepting requests.
    pub healthy_instances: u32,
    /// Total in-flight sequences across all instances.
    pub total_active_sequences: u32,
    /// Total queue depth across all instances.
    pub total_queue_depth: u32,
    /// Total requests routed since startup.
    pub total_requests_routed: u64,
    /// Total requests rejected (backpressure, no candidates).
    pub total_requests_rejected: u64,
    /// Average routing latency in microseconds.
    pub avg_routing_latency_us: u64,
    /// Number of GPUs in the topology (0 if topology not configured).
    pub topology_gpu_count: u32,
    /// Number of NVLink cliques detected.
    pub topology_nvlink_cliques: u32,
    /// Whether topology anomalies are active.
    pub topology_has_anomalies: bool,
    /// KV cache: active sequences tracked by KV manager.
    pub kv_active_sequences: u32,
    /// KV cache: bytes currently in use.
    pub kv_used_bytes: u64,
    /// KV cache: total budget bytes.
    pub kv_total_bytes: u64,
    // Batch metrics ---
    /// Average batch size across all instances (rolling window).
    pub avg_batch_size: f64,
    /// Average batch utilization across all instances.
    pub avg_batch_utilization: f64,
    /// Total sequences waiting for batch admission.
    pub total_batch_waiting: u32,
    /// Admission rate: requests admitted / requests queued (0.0..1.0).
    pub batch_admission_rate: f64,
}

// ---------------------------------------------------------------------------
// Model alias types
// ---------------------------------------------------------------------------

/// A model alias mapping. Maps a user-facing name to a backend model
/// with preferred backends and fallback chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAlias {
    /// The backend model identifier (e.g., "llama3-8b", "qwen3-70b").
    pub model: String,
    /// Ordered list of preferred adapter types. The alias resolver tries
    /// these in order and falls back to any available instance.
    pub preferred_backends: Vec<String>,
}

// ---------------------------------------------------------------------------
// Scheduler configuration
// ---------------------------------------------------------------------------

/// Top-level scheduler configuration, loaded from scheduler.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Placement strategy name. "least-loaded" for Phase 1.
    /// adds "multi-factor".
    #[serde(default = "default_strategy")]
    pub strategy: String,
    /// Maximum queue depth per instance before it's considered overloaded.
    /// Overloaded instances are skipped during placement.
    #[serde(default = "default_overload_threshold")]
    pub overload_queue_threshold: u32,
    /// Global maximum total queue depth. When reached, background requests
    /// are rejected.
    #[serde(default = "default_max_total_queue")]
    pub max_total_queue_depth: u32,
    /// Model alias mappings.
    #[serde(default)]
    pub aliases: HashMap<String, ModelAlias>,
}

fn default_strategy() -> String {
    "least-loaded".to_string()
}

fn default_overload_threshold() -> u32 {
    64
}

fn default_max_total_queue() -> u32 {
    512
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            overload_queue_threshold: default_overload_threshold(),
            max_total_queue_depth: default_max_total_queue(),
            aliases: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scoring function type (pluggable)
// ---------------------------------------------------------------------------

/// Scoring function signature. Takes an instance state and a schedule request,
/// returns a score. Lower score = better candidate.
///
/// This is the extension point replaces. Phase 1 uses a simple
/// least-loaded scorer. The function is stored as a `Box<dyn Fn>` in the
/// placement engine, making it replaceable at runtime.
pub type ScoringFn = Box<dyn Fn(&InstanceState, &ScheduleRequest) -> f64 + Send + Sync>;

/// Optional diagnostic companion to a scoring function.
///
/// Returns a compact human-readable breakdown for the chosen placement.
pub type ScoringReasonFn = Box<dyn Fn(&InstanceState, &ScheduleRequest) -> String + Send + Sync>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Scheduler errors.
#[derive(Error, Debug)]
pub enum SchedulerError {
    /// No instance serves the requested model.
    #[error("no instance available for model '{0}'")]
    NoInstanceAvailable(String),

    /// Model alias not found in configuration.
    #[error("unknown model alias '{0}'")]
    UnknownAlias(String),

    /// System overloaded: total queue depth exceeded.
    #[error("system overloaded: {0} requests queued (max {1})")]
    SystemOverloaded(u32, u32),

    /// Instance not found in registry.
    #[error("instance '{0}' not found")]
    InstanceNotFound(InstanceId),

    /// Duplicate instance registration.
    #[error("instance '{0}' already registered")]
    DuplicateInstance(InstanceId),

    /// Configuration error.
    #[error("config error: {0}")]
    ConfigError(String),
}
