//! MAI Inference Adapter Trait
//!
//! Defines the untrusted capsule interface for backend inference engines.
//! Adapters implementing this trait MUST NOT access hardware directly.
//! All resource requests must route through the HIL.
//!
//! Corrected per Claude audit (B1, B2, B3).

#![deny(unsafe_code)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Core Trait ───────────────────────────────────────────────────────────────

/// Core trait all backend adapters must implement.
/// Works identically for GPU-era and QM-era backends.
///
/// # Trust Model
/// Adapters are untrusted capsules. They run in isolated OS processes with
/// cgroups limits. The `hil_handle` is the ONLY path to hardware resources.
#[async_trait]
pub trait InferenceAdapter: Send + Sync {
    /// Initialize adapter with configuration and HIL access handle.
    /// Called once per adapter lifecycle. Blocks until backend is ready.
    /// Returns an opaque handle string identifying this adapter instance.
    async fn initialize(
        &mut self,
        config: AdapterConfig,
        hil_handle: Arc<dyn HILHandle>,
    ) -> Result<AdapterHandle, AdapterError>;

    /// Generate tokens for a single prompt. Returns async stream of tokens.
    /// Backpressure managed via channel capacity in the FFI bridge.
    fn generate(
        &self,
        prompt: String,
        params: GenerationParams,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, AdapterError>> + Send + '_>>;

    /// Generate tokens for multiple prompts concurrently.
    /// Backends without native batching parallelize internally.
    async fn generate_batch(
        &self,
        prompts: Vec<String>,
        params: GenerationParams,
    ) -> Result<Vec<GenerationResult>, AdapterError>;

    /// Compute embeddings for a batch of texts.
    /// Backends without native embedding support MUST return
    /// `AdapterError::UnsupportedOperation`.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, AdapterError>;

    /// Check adapter health. Should be lightweight and fast (<100ms).
    async fn health_check(&self) -> HealthStatus;

    /// Return static capabilities of this adapter.
    /// These are logical capabilities only — no hardware details.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Gracefully shutdown backend and release resources via HIL.
    async fn shutdown(&mut self) -> Result<(), AdapterError>;
}

// ─── Configuration ────────────────────────────────────────────────────────────

/// Adapter configuration loaded from TOML.
/// Backend-specific fields go in `extra` to support per-backend configs
/// without widening this struct for every backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterConfig {
    pub backend_name: String,
    pub host: String,
    pub port: u16,
    pub model_path: String,
    pub max_concurrent_requests: usize,
    pub timeout_ms: u64,
    pub gpu_layers: Option<u32>,
    pub quantization: Option<String>,
    /// Backend-specific configuration fields (keep_alive, tensor_parallel_size, etc.)
    #[serde(default, flatten)]
    pub extra: std::collections::HashMap<String, toml::Value>,
}

// ─── Data Types ───────────────────────────────────────────────────────────────

/// Single token output with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub text: String,
    pub logprob: Option<f32>,
    pub index: usize,
    pub is_end_of_text: bool,
}

/// Generation parameters passed from scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationParams {
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: usize,
    pub stop_sequences: Vec<String>,
    pub structured_schema: Option<serde_json::Value>,
}

/// Batch generation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResult {
    pub text: String,
    pub tokens_generated: usize,
    pub finish_reason: FinishReason,
}

/// Why generation stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    MaxTokens,
    StopSequence,
}

/// Embedding vector output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub input_tokens: usize,
}

// ─── Health & Capabilities ────────────────────────────────────────────────────

/// Adapter health status with associated diagnostic data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy {
        uptime_ms: u64,
        requests_served: u64,
    },
    Degraded {
        reason: String,
        uptime_ms: u64,
    },
    Unavailable,
}

/// Static capabilities reported by adapter.
///
/// IMPORTANT: This struct declares LOGICAL capabilities only.
/// No hardware details. Hardware acceleration type, VRAM budgets, and
/// measured latency are the HIL's and AdapterManager's responsibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    /// Maximum context window in tokens.
    pub max_context_window: usize,
    /// Quantization formats this adapter can load.
    pub supported_quantizations: Vec<String>,
    /// Whether this adapter supports streaming token output.
    pub supports_streaming: bool,
    /// Whether this adapter supports batch generation.
    pub supports_batching: bool,
    /// Whether this adapter supports JSON schema / grammar constrained output.
    pub supports_structured_output: bool,
    /// Whether this adapter supports vision/multimodal inputs.
    pub supports_vision: bool,
    /// Whether this adapter supports tool/function calling.
    pub supports_tool_calling: bool,
    /// Whether this adapter supports continuous (inflight) batching.
    pub supports_continuous_batching: bool,
    /// Whether this adapter supports embedding computation.
    pub supports_embedding: bool,
    /// Whether this adapter can be hot-swapped without downtime.
    pub supports_hot_swap: bool,
    /// Backend engine version string (informational).
    pub backend_version: String,
}

// ─── Error Taxonomy ───────────────────────────────────────────────────────────

/// Standardized error taxonomy for all adapters.
/// All Python exceptions MUST map to one of these variants before crossing FFI.
#[derive(Error, Debug, Serialize, Deserialize)]
pub enum AdapterError {
    #[error("Backend request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("Out of memory on backend")]
    OutOfMemory,

    #[error("Model '{model}' not found or not loaded")]
    ModelNotFound { model: String },

    #[error("Backend process crashed unexpectedly")]
    BackendCrashed,

    #[error("Backend service unavailable")]
    BackendUnavailable,

    #[error("Prompt exceeds max context window of {max_context} tokens")]
    ContextExceeded { max_context: usize },

    #[error("Backend rate limited")]
    RateLimited,

    #[error("Hardware fault reported via HIL: {detail}")]
    HardwareFault { detail: String },

    #[error("Configuration or schema validation failed: {reason}")]
    ValidationError { reason: String },

    #[error("Operation not supported by this backend: {operation}")]
    UnsupportedOperation { operation: String },
}

impl AdapterError {
    /// Whether the AdapterManager should attempt automatic retry.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AdapterError::Timeout { .. }
                | AdapterError::BackendCrashed
                | AdapterError::BackendUnavailable
                | AdapterError::RateLimited
        )
    }
}

// ─── HIL Handle ───────────────────────────────────────────────────────────────

/// HIL handle provided to adapters for resource requests.
/// This is the ONLY interface an adapter has to hardware resources.
#[async_trait]
pub trait HILHandle: Send + Sync {
    /// Request VRAM allocation. Returns handle on success.
    async fn request_memory(&self, bytes: usize) -> Result<MemoryAllocation, AdapterError>;

    /// Release a previously allocated memory region.
    async fn release_memory(&self, allocation: MemoryAllocation) -> Result<(), AdapterError>;

    /// Request secure model weight loading from vault.
    async fn secure_load_model(&self, model_id: &str) -> Result<ModelLoadHandle, AdapterError>;

    /// Report current thermal state of assigned compute.
    async fn thermal_state(&self) -> ThermalState;

    /// Report telemetry metrics to the kernel.
    async fn report_metrics(&self, metrics: AdapterMetrics) -> Result<(), AdapterError>;
}

/// Opaque memory allocation handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAllocation {
    pub handle: u64,
    pub size_bytes: usize,
}

/// Opaque model load handle.
#[derive(Debug, Clone)]
pub struct ModelLoadHandle {
    pub model_id: String,
    pub ready: bool,
}

/// Thermal state of assigned compute resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThermalState {
    Normal,
    Warning,
    Throttled,
}

/// Telemetry metrics an adapter reports per request or interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterMetrics {
    pub tokens_in: usize,
    pub tokens_out: usize,
    pub latency_ms: f64,
    pub queue_depth: usize,
}

/// Opaque adapter instance handle.
pub type AdapterHandle = String;
