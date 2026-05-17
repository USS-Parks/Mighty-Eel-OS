//! Model Scheduler - Request routing and load balancing
//!
//! Routes inference requests to the optimal adapter+model combination based on
//! configurable strategies, hardware capabilities, and family profile priorities.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::RwLock;

use crate::types::{AdapterId, GpuIdentifier, ModelId, ProfileId, RequestId};
use mai_hil::traits::{AdapterCapabilities, AdapterHandle, HardwareProbe, MemoryManager};

/// Configurable scheduling strategies
#[derive(Debug, Clone)]
pub enum SchedulingStrategy {
    /// Distribute requests evenly across available adapters
    RoundRobin,
    /// Route to adapter with lowest current load (requests in flight)
    LeastLoaded,
    /// Prefer adapter that already has the model loaded (reduce VRAM churn)
    ModelAffinity,
    /// Priority queue: higher-priority profiles jump the queue
    PriorityQueued,
    /// Hybrid: primary strategy first, secondary as tiebreaker
    Hybrid {
        primary: Box<SchedulingStrategy>,
        secondary: Box<SchedulingStrategy>,
    },
}

/// Priority levels derived from family profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequestPriority {
    /// Guest profiles, background tasks
    Low,
    /// Default for adult profiles
    Normal,
    /// Admin profiles, interactive chat
    High,
    /// System tasks, wake triggers
    Critical,
}

/// Types of inference requests
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    /// Multi-turn conversation
    Chat,
    /// Single-shot text completion
    Completion,
    /// Vector embedding computation
    Embedding,
    /// JSON schema / grammar constrained output
    Structured,
    /// Tool/function calling
    FunctionCall,
}

/// Unified request payload
#[derive(Debug, Clone)]
pub enum RequestPayload {
    /// Chat messages (multi-turn)
    Chat { messages: Vec<ChatMessage> },
    /// Single prompt completion
    Completion { prompt: String },
    /// Texts to embed
    Embedding { texts: Vec<String> },
}

/// Single chat message
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Role: "user", "assistant", "system"
    pub role: String,
    /// Message content
    pub content: String,
}

/// Full inference request structure
#[derive(Debug, Clone)]
pub struct InferenceRequest {
    /// Unique request identifier
    pub id: RequestId,
    /// Family profile for auth/priority
    pub profile_id: ProfileId,
    /// Target model name
    pub model_name: String,
    /// Request type
    pub request_type: RequestType,
    /// Request payload
    pub payload: RequestPayload,
    /// Priority (derived from profile + explicit header)
    pub priority: RequestPriority,
    /// Per-request timeout override
    pub timeout: Option<Duration>,
    /// Whether to stream tokens
    pub streaming: bool,
}

/// Scheduler configuration
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Active scheduling strategy
    pub strategy: SchedulingStrategy,
    /// Max queue depth per priority level
    pub max_queue_depth_per_priority: HashMap<RequestPriority, usize>,
    /// Default request timeout
    pub default_timeout: Duration,
    /// Queue utilization threshold for backpressure (0.0-1.0)
    pub backpressure_threshold: f64,
    /// Whether Sentinel promotion is enabled
    pub sentinel_promotion_enabled: bool,
}

/// Result of adapter selection
#[derive(Debug)]
pub struct AdapterSelection {
    /// Selected adapter
    pub adapter_id: AdapterId,
    /// Selected model
    pub model_id: ModelId,
    /// GPU assignment (None for CPU fallback)
    pub gpu_assignment: Option<GpuIdentifier>,
    /// Estimated request latency
    pub estimated_latency: Duration,
}

/// Scheduler errors
#[derive(Error, Debug)]
pub enum SchedulerError {
    /// No adapter supports the requested model/capabilities
    #[error("No compatible adapter found for model {0}")]
    NoCompatibleAdapter(String),

    /// All adapters at maximum request capacity
    #[error("All adapters at capacity")]
    AllAdaptersBusy,

    /// Request exceeded timeout
    #[error("Request timeout after {0:?}")]
    Timeout(Duration),

    /// Queue full for the given priority level
    #[error("Queue full for priority {0:?}")]
    QueueFull(RequestPriority),

    /// HIL layer error (wrapped)
    #[error("HIL error: {0}")]
    HilError(String),

    /// Adapter layer error (wrapped)
    #[error("Adapter error: {0}")]
    AdapterError(String),
}

/// Main scheduler struct
pub struct Scheduler {
    config: SchedulerConfig,
    adapters: Arc<RwLock<HashMap<AdapterId, AdapterInfo>>>,
    models: Arc<RwLock<HashMap<ModelId, ModelPlacement>>>,
    hardware_probe: Arc<dyn HardwareProbe>,
    memory_manager: Arc<dyn MemoryManager>,
}

struct AdapterInfo {
    _handle: AdapterHandle,
    _capabilities: AdapterCapabilities,
    current_load: usize,
    health_status: bool,
}

struct ModelPlacement {
    _adapter_id: AdapterId,
    gpu_id: Option<GpuIdentifier>,
    vram_allocated: u64,
}

impl Scheduler {
    /// Create a new scheduler with configuration and HIL dependencies
    pub fn new(
        config: SchedulerConfig,
        hardware_probe: Arc<dyn HardwareProbe>,
        memory_manager: Arc<dyn MemoryManager>,
    ) -> Self {
        Self {
            config,
            adapters: Arc::new(RwLock::new(HashMap::new())),
            models: Arc::new(RwLock::new(HashMap::new())),
            hardware_probe,
            memory_manager,
        }
    }

    /// Register an adapter with the scheduler
    pub async fn register_adapter(
        &self,
        _adapter_id: AdapterId,
        _handle: AdapterHandle,
        _capabilities: AdapterCapabilities,
    ) -> Result<(), SchedulerError> {
        // Implementation in Session 07
        todo!()
    }

    /// Route a request to the optimal adapter+model
    pub async fn route_request(
        &self,
        _request: InferenceRequest,
    ) -> Result<AdapterSelection, SchedulerError> {
        // Implementation in Session 07
        todo!()
    }

    /// Evaluate if request exceeds Sentinel capability (for promotion)
    pub fn evaluate_complexity(&self, _request: &InferenceRequest) -> ComplexityScore {
        // Implementation in Session 07
        todo!()
    }

    /// Trigger promotion to Full Inference mode
    pub async fn promote_to_full_inference(
        &self,
        _request_id: RequestId,
        _required_capabilities: ModelCapabilities,
    ) -> Result<PromotionResult, SchedulerError> {
        // Implementation in Session 07
        todo!()
    }

    /// Apply backpressure when queues are full
    pub fn apply_backpressure(&self, _priority: RequestPriority) -> Option<BackpressureAction> {
        // Implementation in Session 07
        todo!()
    }

    /// Get current queue depth for monitoring
    pub async fn queue_depth(&self, _priority: RequestPriority) -> usize {
        // Implementation in Session 07
        todo!()
    }
}

/// Complexity assessment for Sentinel promotion decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplexityScore {
    /// Handled by Sentinel (Q&A, commands, reminders)
    Simple,
    /// May exceed Sentinel context or reasoning
    Moderate,
    /// Requires Full model (long context, multi-step, embedding)
    Complex,
}

/// Capabilities required by a request (for promotion evaluation)
#[derive(Debug, Clone)]
pub struct ModelCapabilities {
    /// Minimum context window needed
    pub min_context_tokens: u32,
    /// Whether structured output is required
    pub requires_structured_output: bool,
    /// Whether vision/multimodal is required
    pub requires_vision: bool,
    /// Whether tool calling is required
    pub requires_tool_calling: bool,
}

/// Result of promotion attempt
#[derive(Debug)]
pub enum PromotionResult {
    /// Full model loaded and serving
    Success { first_token_latency: Duration },
    /// Promotion not possible, falling back to Sentinel
    FallbackToSentinel { reason: String },
    /// Promotion failed with error
    Failed { error: SchedulerError },
}

/// Backpressure actions when queue utilization exceeds threshold
#[derive(Debug, Clone, Copy)]
pub enum BackpressureAction {
    /// Reject new requests at or below this priority
    RejectNew(RequestPriority),
    /// Extend timeout for this priority level
    ExtendTimeout(RequestPriority),
    /// Signal client with retry-after header
    SignalClient { retry_after_seconds: u32 },
}
