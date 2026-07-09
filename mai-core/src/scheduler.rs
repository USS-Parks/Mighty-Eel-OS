//! Model Scheduler - Request routing, load balancing, and Sentinel promotion
//!
//! Routes inference requests to the best available adapter based on model
//! capabilities, load balancing strategy, and GPU VRAM availability. Detects
//! when requests exceed Sentinel model capability and triggers promotion
//! to Full Inference.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use crate::types::{AdapterId, GpuIdentifier, ModelId, ProfileId, RequestId};

/// Scheduling strategy for adapter selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchedulingStrategy {
    /// Cycle through adapters in order
    RoundRobin,
    /// Pick the adapter with fewest in-flight requests
    LeastLoaded,
    /// Prefer the adapter that already has the model loaded
    ModelAffinity,
    /// Route based on request priority level
    PriorityQueued,
    /// Combine two strategies. Max recursion depth: 3.
    Hybrid {
        primary: Box<SchedulingStrategy>,
        fallback: Box<SchedulingStrategy>,
        /// Nesting depth (enforced, max 3)
        depth: u8,
    },
}

impl SchedulingStrategy {
    /// Validate that Hybrid nesting doesn't exceed max depth.
    pub fn validate(&self) -> Result<(), SchedulerError> {
        match self {
            Self::Hybrid {
                primary,
                fallback,
                depth,
            } => {
                if *depth > 3 {
                    return Err(SchedulerError::ConfigError(
                        "Hybrid strategy max nesting depth is 3".to_string(),
                    ));
                }
                primary.validate()?;
                fallback.validate()?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Request priority levels (family profile-based)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RequestPriority {
    /// Background tasks, batch processing
    Low = 0,
    /// Standard user requests
    Normal = 1,
    /// Elevated (e.g., parent profile, time-sensitive)
    High = 2,
    /// System-critical (health checks, security tasks)
    Critical = 3,
}

/// Scheduler lifecycle events for audit/health reporting
#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    /// Circuit breaker tripped on an adapter
    CircuitTripped {
        adapter_id: AdapterId,
        state: CircuitState,
        cooldown: Option<Duration>,
    },
    /// Adapter circuit breaker recovered to Closed
    CircuitRecovered { adapter_id: AdapterId },
}

/// Type of inference request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestType {
    Chat,
    Completion,
    Embedding,
    Structured,
    FunctionCall,
}

/// Request payload variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequestPayload {
    Chat { messages: Vec<ChatMessage> },
    Completion { prompt: String },
    Embedding { texts: Vec<String> },
}

/// Single chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// A queued inference request
#[derive(Debug, Clone)]
pub struct InferenceRequest {
    /// Unique request identifier
    pub id: RequestId,
    /// Requesting family profile
    pub profile_id: ProfileId,
    /// Requested model (empty = scheduler picks)
    pub model_name: Option<ModelId>,
    /// Type of request
    pub request_type: RequestType,
    /// Request payload
    pub payload: RequestPayload,
    /// Priority level
    pub priority: RequestPriority,
    /// Per-request timeout
    pub timeout: Duration,
    /// Whether to stream response tokens
    pub streaming: bool,
    /// When the request was enqueued
    pub enqueued_at: Instant,
    /// Estimated token count for complexity assessment
    pub estimated_tokens: u32,
}

/// Scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Primary scheduling strategy
    pub strategy: SchedulingStrategy,
    /// Max queue depth per priority level
    pub max_queue_depth_per_priority: HashMap<String, usize>,
    /// Default request timeout
    pub default_timeout: Duration,
    /// Queue depth at which backpressure activates
    pub backpressure_threshold: usize,
    /// Whether Sentinel promotion is enabled
    pub sentinel_promotion_enabled: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        let mut max_depth = HashMap::new();
        max_depth.insert("Low".to_string(), 100);
        max_depth.insert("Normal".to_string(), 50);
        max_depth.insert("High".to_string(), 25);
        max_depth.insert("Critical".to_string(), 10);

        Self {
            strategy: SchedulingStrategy::LeastLoaded,
            max_queue_depth_per_priority: max_depth,
            default_timeout: Duration::from_secs(120),
            backpressure_threshold: 80,
            sentinel_promotion_enabled: true,
        }
    }
}

/// Result of adapter selection
#[derive(Debug, Clone)]
pub struct AdapterSelection {
    /// Selected adapter
    pub adapter_id: AdapterId,
    /// Model to use on that adapter
    pub model_id: ModelId,
    /// GPU assigned for this request
    pub gpu_id: Option<GpuIdentifier>,
    /// Whether Sentinel promotion was triggered
    pub promotion_triggered: bool,
}

/// Scheduler errors
#[derive(Error, Debug)]
pub enum SchedulerError {
    /// No adapter available for this request
    #[error("No adapter available for model {0}")]
    NoAdapterAvailable(String),

    /// Request queue is full (backpressure)
    #[error("Queue full: {0} requests pending at priority {1}")]
    QueueFull(usize, String),

    /// Request timed out waiting in queue
    #[error("Request {0} timed out after {1}ms")]
    RequestTimeout(String, u64),

    /// Model not loaded and cannot be loaded
    #[error("Model not loadable: {0}")]
    ModelNotLoadable(String),

    /// Configuration error
    #[error("Config error: {0}")]
    ConfigError(String),
}

/// Complexity assessment for Sentinel promotion decisions
#[derive(Debug, Clone)]
pub struct ComplexityScore {
    /// Estimated input tokens
    pub input_tokens: u32,
    /// Estimated output tokens
    pub output_tokens: u32,
    /// Whether task requires multi-step reasoning
    pub is_complex_task: bool,
    /// Whether task requires vision/multimodal
    pub requires_vision: bool,
    /// Whether task requires tool calling
    pub requires_tool_calling: bool,
}

impl ComplexityScore {
    /// Whether this complexity exceeds Sentinel model capability
    pub fn exceeds_sentinel(&self) -> bool {
        self.input_tokens > 4096
            || self.output_tokens > 2048
            || self.is_complex_task
            || self.requires_vision
            || self.requires_tool_calling
    }
}

/// Registered adapter info tracked by the scheduler
#[derive(Debug)]
struct AdapterInfo {
    adapter_id: AdapterId,
    /// Models this adapter can serve
    supported_models: Vec<ModelId>,
    /// Current in-flight request count
    in_flight: usize,
    /// Maximum concurrent requests
    max_concurrent: usize,
    /// GPU(s) assigned to this adapter
    gpu_ids: Vec<GpuIdentifier>,
    /// Whether adapter is healthy (HealthMonitor signal)
    is_healthy: bool,
    /// Circuit breaker for partial failure tracking
    circuit_breaker: CircuitBreaker,
    /// Last time a request was routed here
    last_used: Instant,
}

/// Local telemetry metrics (never transmitted off-device)
#[derive(Debug, Clone, Default)]
pub struct SchedulerMetrics {
    /// Total requests routed
    pub total_routed: u64,
    /// Total requests rejected (backpressure)
    pub total_rejected: u64,
    /// Total Sentinel promotions triggered
    pub total_promotions: u64,
    /// Total request timeouts
    pub total_timeouts: u64,
    /// Average queue depth over measurement window
    pub avg_queue_depth: f32,
    /// Average routing latency in ms
    pub avg_routing_latency_ms: f64,
}

/// Backpressure action when queue fills
#[derive(Debug, Clone)]
pub enum BackpressureAction {
    /// Accept request normally
    Accept,
    /// Reject lowest-priority requests
    RejectLowPriority,
    /// Reject all non-critical requests
    RejectNonCritical,
    /// Reject everything (system overloaded)
    RejectAll,
}

/// Result of Sentinel promotion evaluation
#[derive(Debug, Clone)]
pub enum PromotionResult {
    /// Sentinel can handle this request
    SentinelSufficient,
    /// Need Full Inference; promotion triggered
    PromotionTriggered,
    /// Already in Full Inference mode
    AlreadyFull,
}

/// The model scheduler. Routes requests to adapters.
pub struct Scheduler {
    config: SchedulerConfig,
    /// Registered adapters
    adapters: HashMap<AdapterId, AdapterInfo>,
    /// Priority queues (one per priority level)
    queues: HashMap<RequestPriority, VecDeque<InferenceRequest>>,
    /// Round-robin index for RoundRobin strategy
    round_robin_index: usize,
    /// Local metrics (never transmitted)
    metrics: SchedulerMetrics,
    /// Pending lifecycle events for circuit breaker state changes
    pub pending_events: Vec<SchedulerEvent>,
}

impl Scheduler {
    /// Create a new scheduler with the given configuration
    pub fn new(config: SchedulerConfig) -> Result<Self, SchedulerError> {
        config.strategy.validate()?;

        let mut queues = HashMap::new();
        queues.insert(RequestPriority::Low, VecDeque::new());
        queues.insert(RequestPriority::Normal, VecDeque::new());
        queues.insert(RequestPriority::High, VecDeque::new());
        queues.insert(RequestPriority::Critical, VecDeque::new());

        Ok(Self {
            config,
            adapters: HashMap::new(),
            queues,
            round_robin_index: 0,
            metrics: SchedulerMetrics::default(),
            pending_events: Vec::new(),
        })
    }

    /// Register an adapter with the scheduler
    pub fn register_adapter(
        &mut self,
        adapter_id: AdapterId,
        supported_models: Vec<ModelId>,
        max_concurrent: usize,
        gpu_ids: Vec<GpuIdentifier>,
    ) {
        info!(
            adapter = %adapter_id,
            models = ?supported_models,
            max_concurrent = max_concurrent,
            "Registering adapter with scheduler"
        );
        self.adapters.insert(
            adapter_id.clone(),
            AdapterInfo {
                adapter_id,
                supported_models,
                in_flight: 0,
                max_concurrent,
                gpu_ids,
                is_healthy: true,
                circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
                last_used: Instant::now(),
            },
        );
    }

    /// Unregister an adapter (e.g., on adapter shutdown)
    pub fn unregister_adapter(&mut self, adapter_id: &AdapterId) {
        self.adapters.remove(adapter_id);
    }

    /// Mark an adapter as healthy or unhealthy, with circuit breaker interop
    pub fn set_adapter_health(&mut self, adapter_id: &AdapterId, healthy: bool) {
        if let Some(adapter) = self.adapters.get_mut(adapter_id) {
            adapter.is_healthy = healthy;
            if healthy {
                adapter.circuit_breaker.reset();
                self.pending_events.push(SchedulerEvent::CircuitRecovered {
                    adapter_id: adapter_id.clone(),
                });
                info!(adapter_id = %adapter_id, "Health restored, circuit breaker reset to Closed");
            } else {
                let was_closed = adapter.circuit_breaker.state() == CircuitState::Closed;
                adapter.circuit_breaker.force_open();
                if was_closed {
                    let cooldown = adapter.circuit_breaker.time_until_half_open();
                    self.pending_events.push(SchedulerEvent::CircuitTripped {
                        adapter_id: adapter_id.clone(),
                        state: CircuitState::Open,
                        cooldown,
                    });
                    warn!(adapter_id = %adapter_id, "HealthMonitor declared dead, circuit forced Open");
                }
            }
        }
    }

    /// Route a request to the best available adapter.
    /// Returns the adapter selection or an error if no adapter is available.
    pub fn route_request(
        &mut self,
        request: &InferenceRequest,
    ) -> Result<AdapterSelection, SchedulerError> {
        let started = Instant::now();

        // Check backpressure
        let action = self.evaluate_backpressure();
        match action {
            BackpressureAction::RejectAll => {
                self.metrics.total_rejected += 1;
                return Err(SchedulerError::QueueFull(
                    self.total_queue_depth(),
                    format!("{:?}", request.priority),
                ));
            }
            BackpressureAction::RejectNonCritical
                if request.priority != RequestPriority::Critical =>
            {
                self.metrics.total_rejected += 1;
                return Err(SchedulerError::QueueFull(
                    self.total_queue_depth(),
                    format!("{:?}", request.priority),
                ));
            }
            BackpressureAction::RejectLowPriority if request.priority == RequestPriority::Low => {
                self.metrics.total_rejected += 1;
                return Err(SchedulerError::QueueFull(
                    self.total_queue_depth(),
                    format!("{:?}", request.priority),
                ));
            }
            _ => {}
        }

        // Find candidate adapters for the request
        let candidates = self.find_candidates(request);
        if candidates.is_empty() {
            return Err(SchedulerError::NoAdapterAvailable(
                request
                    .model_name
                    .clone()
                    .unwrap_or_else(|| "any".to_string()),
            ));
        }

        // Select adapter based on strategy
        let selected = self.select_adapter(&candidates, request)?;

        // Update metrics
        if let Some(adapter) = self.adapters.get_mut(&selected.adapter_id) {
            adapter.in_flight += 1;
            adapter.last_used = Instant::now();
        }

        self.metrics.total_routed += 1;
        let latency = started.elapsed().as_secs_f64() * 1000.0;
        // Running average
        // Safety: u64 counter -> f64 loses precision only above 2^53, acceptable for averaging
        #[allow(clippy::cast_precision_loss)]
        let n = self.metrics.total_routed as f64;
        self.metrics.avg_routing_latency_ms =
            self.metrics.avg_routing_latency_ms * ((n - 1.0) / n) + latency / n;

        debug!(
            request_id = %request.id,
            adapter = %selected.adapter_id,
            model = %selected.model_id,
            "Request routed"
        );

        Ok(selected)
    }

    /// Notify scheduler that a request completed (decrement in-flight count)
    pub fn request_completed(&mut self, adapter_id: &AdapterId) {
        if let Some(adapter) = self.adapters.get_mut(adapter_id) {
            adapter.in_flight = adapter.in_flight.saturating_sub(1);
        }
    }

    /// Evaluate request complexity for Sentinel promotion decision
    pub fn evaluate_complexity(&self, request: &InferenceRequest) -> ComplexityScore {
        let input_tokens = request.estimated_tokens;

        let is_complex = matches!(
            request.request_type,
            RequestType::FunctionCall | RequestType::Structured
        );

        // TODO(basho): inspect the request payload for image/vision parts; until
        // multimodal payloads are wired this conservatively assumes text-only.
        let requires_vision = false;

        ComplexityScore {
            input_tokens,
            // Heuristic pre-inference estimate for the promotion decision — the true
            // output length is only known after generation, so this is intentionally
            // approximate, not a reported token count.
            output_tokens: input_tokens / 2,
            is_complex_task: is_complex,
            requires_vision,
            requires_tool_calling: request.request_type == RequestType::FunctionCall,
        }
    }

    /// Check if Sentinel promotion is needed for a request
    pub fn check_promotion(&mut self, request: &InferenceRequest) -> PromotionResult {
        if !self.config.sentinel_promotion_enabled {
            return PromotionResult::AlreadyFull;
        }

        let complexity = self.evaluate_complexity(request);
        if complexity.exceeds_sentinel() {
            self.metrics.total_promotions += 1;
            info!(
                request_id = %request.id,
                input_tokens = complexity.input_tokens,
                is_complex = complexity.is_complex_task,
                "Sentinel promotion triggered"
            );
            PromotionResult::PromotionTriggered
        } else {
            PromotionResult::SentinelSufficient
        }
    }

    /// Get current backpressure action
    pub fn evaluate_backpressure(&self) -> BackpressureAction {
        let total = self.total_queue_depth();
        let threshold = self.config.backpressure_threshold;

        if total < threshold {
            BackpressureAction::Accept
        } else if total < threshold * 2 {
            BackpressureAction::RejectLowPriority
        } else if total < threshold * 3 {
            BackpressureAction::RejectNonCritical
        } else {
            BackpressureAction::RejectAll
        }
    }

    /// Total requests across all queues + in-flight
    pub fn total_queue_depth(&self) -> usize {
        let queued: usize = self.queues.values().map(VecDeque::len).sum();
        let in_flight: usize = self.adapters.values().map(|a| a.in_flight).sum();
        queued + in_flight
    }

    /// Get current scheduler metrics (local-only telemetry)
    pub fn metrics(&self) -> &SchedulerMetrics {
        &self.metrics
    }

    /// Number of registered adapters
    pub fn adapter_count(&self) -> usize {
        self.adapters.len()
    }

    /// Number of healthy adapters
    pub fn healthy_adapter_count(&self) -> usize {
        self.adapters.values().filter(|a| a.is_healthy).count()
    }

    /// Number of in-flight requests for a specific adapter (0 if unknown).
    pub fn adapter_in_flight(&self, adapter_id: &AdapterId) -> usize {
        self.adapters.get(adapter_id).map_or(0, |a| a.in_flight)
    }

    /// Record a successful response from an adapter (updates circuit breaker)
    pub fn record_adapter_success(&mut self, adapter_id: &AdapterId) {
        if let Some(adapter) = self.adapters.get_mut(adapter_id) {
            let was_half_open = adapter.circuit_breaker.state() == CircuitState::HalfOpen;
            adapter.circuit_breaker.record_success();
            if was_half_open && adapter.circuit_breaker.state() == CircuitState::Closed {
                self.pending_events.push(SchedulerEvent::CircuitRecovered {
                    adapter_id: adapter_id.clone(),
                });
                info!(adapter_id = %adapter_id, "Circuit breaker recovered via probe success");
            }
        }
    }

    /// Record a failed response from an adapter (updates circuit breaker)
    pub fn record_adapter_failure(&mut self, adapter_id: &AdapterId) {
        if let Some(adapter) = self.adapters.get_mut(adapter_id) {
            let prev_state = adapter.circuit_breaker.state();
            adapter.circuit_breaker.record_failure();
            let new_state = adapter.circuit_breaker.state();

            if prev_state != CircuitState::Open && new_state == CircuitState::Open {
                let cooldown = adapter.circuit_breaker.time_until_half_open();
                self.pending_events.push(SchedulerEvent::CircuitTripped {
                    adapter_id: adapter_id.clone(),
                    state: CircuitState::Open,
                    cooldown,
                });
                let metrics = adapter.circuit_breaker.metrics();
                warn!(
                    adapter_id = %adapter_id,
                    consecutive_failures = metrics.consecutive_failures,
                    failures_in_window = metrics.failures_in_window,
                    total_in_window = metrics.total_in_window,
                    "Circuit breaker tripped"
                );
            }
        }
    }

    /// Consume pending events (call periodically or after route_request)
    pub fn drain_events(&mut self) -> Vec<SchedulerEvent> {
        std::mem::take(&mut self.pending_events)
    }

    // ─── Internal helpers ─────────────────────────────────────────────

    /// Find adapters that can serve a request (checks health, circuit state, capacity)
    fn find_candidates(&mut self, request: &InferenceRequest) -> Vec<AdapterId> {
        self.adapters
            .values_mut()
            .filter_map(|a| {
                // Must be healthy (HealthMonitor signal)
                if !a.is_healthy {
                    return None;
                }
                // Refresh circuit state (Open -> HalfOpen if cooldown elapsed)
                a.circuit_breaker.refresh_state();
                // Circuit must allow execution
                if !a.circuit_breaker.can_execute() {
                    return None;
                }
                // Must have capacity
                if a.in_flight >= a.max_concurrent {
                    return None;
                }
                // Must support the requested model (if specified)
                if let Some(ref model) = request.model_name
                    && !a.supported_models.contains(model)
                {
                    return None;
                }
                Some(a.adapter_id.clone())
            })
            .collect()
    }

    /// Select the best adapter from candidates based on strategy
    fn select_adapter(
        &mut self,
        candidates: &[AdapterId],
        request: &InferenceRequest,
    ) -> Result<AdapterSelection, SchedulerError> {
        let selected_id = match &self.config.strategy {
            SchedulingStrategy::RoundRobin => {
                let idx = self.round_robin_index % candidates.len();
                self.round_robin_index = self.round_robin_index.wrapping_add(1);
                candidates[idx].clone()
            }
            SchedulingStrategy::LeastLoaded => candidates
                .iter()
                .min_by_key(|id| self.adapters.get(*id).map_or(usize::MAX, |a| a.in_flight))
                .cloned()
                .ok_or_else(|| SchedulerError::NoAdapterAvailable("no candidates".to_string()))?,
            SchedulingStrategy::ModelAffinity => {
                // Prefer adapter that already has the model loaded
                if let Some(ref model) = request.model_name {
                    candidates
                        .iter()
                        .find(|id| {
                            self.adapters
                                .get(*id)
                                .is_some_and(|a| a.supported_models.contains(model))
                        })
                        .cloned()
                        .unwrap_or_else(|| candidates[0].clone())
                } else {
                    candidates[0].clone()
                }
            }
            SchedulingStrategy::PriorityQueued => {
                // For priority queued, pick least loaded among candidates
                // (priority is handled at the queue level, not adapter selection)
                candidates
                    .iter()
                    .min_by_key(|id| self.adapters.get(*id).map_or(usize::MAX, |a| a.in_flight))
                    .cloned()
                    .ok_or_else(|| {
                        SchedulerError::NoAdapterAvailable("no candidates".to_string())
                    })?
            }
            SchedulingStrategy::Hybrid {
                primary, fallback, ..
            } => {
                // Try primary strategy first, fall back if it fails
                self.select_with_strategy(primary, candidates, request)
                    .unwrap_or_else(|_| {
                        self.select_with_strategy(fallback, candidates, request)
                            .unwrap_or_else(|_| candidates[0].clone())
                    })
            }
        };

        let adapter = self
            .adapters
            .get(&selected_id)
            .ok_or_else(|| SchedulerError::NoAdapterAvailable(selected_id.clone()))?;

        let model_id = request
            .model_name
            .clone()
            .or_else(|| adapter.supported_models.first().cloned())
            .unwrap_or_else(|| "default".to_string());

        Ok(AdapterSelection {
            adapter_id: selected_id,
            model_id,
            gpu_id: adapter.gpu_ids.first().cloned(),
            promotion_triggered: false,
        })
    }

    /// Helper for Hybrid strategy: apply a specific strategy to candidates
    #[allow(clippy::only_used_in_recursion)] // request threaded for future strategy use
    fn select_with_strategy(
        &self,
        strategy: &SchedulingStrategy,
        candidates: &[AdapterId],
        request: &InferenceRequest,
    ) -> Result<AdapterId, SchedulerError> {
        match strategy {
            SchedulingStrategy::LeastLoaded | SchedulingStrategy::PriorityQueued => candidates
                .iter()
                .min_by_key(|id| self.adapters.get(*id).map_or(usize::MAX, |a| a.in_flight))
                .cloned()
                .ok_or_else(|| SchedulerError::NoAdapterAvailable("empty".to_string())),
            SchedulingStrategy::RoundRobin | SchedulingStrategy::ModelAffinity => {
                // Can't mutate round_robin_index in &self, use first candidate
                candidates
                    .first()
                    .cloned()
                    .ok_or_else(|| SchedulerError::NoAdapterAvailable("empty".to_string()))
            }
            SchedulingStrategy::Hybrid { primary, .. } => {
                self.select_with_strategy(primary, candidates, request)
            }
        }
    }
}

/// Multi-GPU distribution helper.
/// Assigns models to GPUs based on VRAM availability.
pub fn distribute_to_gpu(
    required_vram: u64,
    gpu_available: &[(GpuIdentifier, u64)],
) -> Option<GpuIdentifier> {
    // Pick the GPU with the most available VRAM that fits the model
    gpu_available
        .iter()
        .filter(|(_, avail)| *avail >= required_vram)
        .max_by_key(|(_, avail)| *avail)
        .map(|(id, _)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(priority: RequestPriority, model: Option<&str>) -> InferenceRequest {
        InferenceRequest {
            id: uuid::Uuid::new_v4(),
            profile_id: uuid::Uuid::new_v4(),
            model_name: model.map(|s| s.to_string()),
            request_type: RequestType::Chat,
            payload: RequestPayload::Chat {
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                }],
            },
            priority,
            timeout: Duration::from_secs(30),
            streaming: true,
            enqueued_at: Instant::now(),
            estimated_tokens: 100,
        }
    }

    fn setup_scheduler() -> Scheduler {
        let mut sched = Scheduler::new(SchedulerConfig::default()).unwrap();
        sched.register_adapter(
            "ollama:0".to_string(),
            vec!["qwen3-14b".to_string(), "phi4-mini".to_string()],
            10,
            vec!["nvidia:rtx5090:0".to_string()],
        );
        sched.register_adapter(
            "vllm:0".to_string(),
            vec!["qwen3-70b".to_string()],
            5,
            vec!["nvidia:h100:0".to_string()],
        );
        sched
    }

    #[test]
    fn test_register_adapter() {
        let sched = setup_scheduler();
        assert_eq!(sched.adapter_count(), 2);
        assert_eq!(sched.healthy_adapter_count(), 2);
    }

    #[test]
    fn test_route_request_to_correct_adapter() {
        let mut sched = setup_scheduler();
        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        let selection = sched.route_request(&req).unwrap();
        assert_eq!(selection.adapter_id, "ollama:0");
        assert_eq!(selection.model_id, "qwen3-14b");
    }

    #[test]
    fn test_route_request_no_adapter() {
        let mut sched = setup_scheduler();
        let req = make_request(RequestPriority::Normal, Some("nonexistent-model"));
        let result = sched.route_request(&req);
        assert!(matches!(result, Err(SchedulerError::NoAdapterAvailable(_))));
    }

    #[test]
    fn test_least_loaded_selection() {
        let mut sched = setup_scheduler();

        // Route 3 requests to ollama (it supports phi4-mini)
        for _ in 0..3 {
            let req = make_request(RequestPriority::Normal, Some("phi4-mini"));
            sched.route_request(&req).unwrap();
        }

        // Both adapters support nothing in common except through default selection
        // But if we request a model both support... let's register a shared model
        sched.register_adapter(
            "llamacpp:0".to_string(),
            vec!["phi4-mini".to_string()],
            10,
            vec![],
        );

        // Now route another phi4-mini request -- should go to llamacpp (0 in-flight)
        let req = make_request(RequestPriority::Normal, Some("phi4-mini"));
        let selection = sched.route_request(&req).unwrap();
        assert_eq!(selection.adapter_id, "llamacpp:0");
    }

    #[test]
    fn test_unhealthy_adapter_excluded() {
        let mut sched = setup_scheduler();
        sched.set_adapter_health(&"ollama:0".to_string(), false);

        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        let result = sched.route_request(&req);
        // ollama is the only adapter for qwen3-14b, and it's unhealthy
        assert!(matches!(result, Err(SchedulerError::NoAdapterAvailable(_))));
    }

    #[test]
    fn test_request_completed_decrements() {
        let mut sched = setup_scheduler();
        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        sched.route_request(&req).unwrap();

        // in_flight should be 1
        assert_eq!(sched.adapters["ollama:0"].in_flight, 1);

        sched.request_completed(&"ollama:0".to_string());
        assert_eq!(sched.adapters["ollama:0"].in_flight, 0);
    }

    #[test]
    fn test_complexity_evaluation() {
        let sched = setup_scheduler();

        let simple_req = make_request(RequestPriority::Normal, None);
        let score = sched.evaluate_complexity(&simple_req);
        assert!(!score.exceeds_sentinel());

        let mut complex_req = make_request(RequestPriority::Normal, None);
        complex_req.estimated_tokens = 5000;
        let score = sched.evaluate_complexity(&complex_req);
        assert!(score.exceeds_sentinel());

        let mut tool_req = make_request(RequestPriority::Normal, None);
        tool_req.request_type = RequestType::FunctionCall;
        let score = sched.evaluate_complexity(&tool_req);
        assert!(score.exceeds_sentinel());
    }

    #[test]
    fn test_backpressure_accept_under_threshold() {
        let sched = setup_scheduler();
        let action = sched.evaluate_backpressure();
        assert!(matches!(action, BackpressureAction::Accept));
    }

    #[test]
    fn test_metrics_increment() {
        let mut sched = setup_scheduler();
        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        sched.route_request(&req).unwrap();
        assert_eq!(sched.metrics().total_routed, 1);
    }

    #[test]
    fn test_hybrid_strategy_validation() {
        // Valid hybrid
        let valid = SchedulingStrategy::Hybrid {
            primary: Box::new(SchedulingStrategy::LeastLoaded),
            fallback: Box::new(SchedulingStrategy::RoundRobin),
            depth: 1,
        };
        assert!(valid.validate().is_ok());

        // Exceeds max depth
        let invalid = SchedulingStrategy::Hybrid {
            primary: Box::new(SchedulingStrategy::LeastLoaded),
            fallback: Box::new(SchedulingStrategy::RoundRobin),
            depth: 4,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_distribute_to_gpu() {
        let gpus = vec![
            ("gpu:0".to_string(), 16_000_000_000u64),
            ("gpu:1".to_string(), 32_000_000_000u64),
        ];

        // Should pick gpu:1 (more available VRAM)
        let result = distribute_to_gpu(10_000_000_000, &gpus);
        assert_eq!(result, Some("gpu:1".to_string()));

        // Nothing fits
        let result = distribute_to_gpu(64_000_000_000, &gpus);
        assert!(result.is_none());
    }

    #[test]
    fn test_unregister_adapter() {
        let mut sched = setup_scheduler();
        assert_eq!(sched.adapter_count(), 2);
        sched.unregister_adapter(&"ollama:0".to_string());
        assert_eq!(sched.adapter_count(), 1);
    }

    #[test]
    fn test_promotion_check() {
        let mut sched = setup_scheduler();

        let simple = make_request(RequestPriority::Normal, None);
        let result = sched.check_promotion(&simple);
        assert!(matches!(result, PromotionResult::SentinelSufficient));

        let mut complex = make_request(RequestPriority::Normal, None);
        complex.estimated_tokens = 8000;
        let result = sched.check_promotion(&complex);
        assert!(matches!(result, PromotionResult::PromotionTriggered));
        assert_eq!(sched.metrics().total_promotions, 1);
    }

    // ─── Circuit Breaker Integration Tests ────────────────────────────

    #[test]
    fn test_scheduler_skips_open_circuit() {
        let mut sched = setup_scheduler();
        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        // Default CircuitBreakerConfig has trip_threshold=5
        for _ in 0..5 {
            sched.record_adapter_failure(&"ollama:0".to_string());
        }
        let result = sched.route_request(&req);
        assert!(matches!(result, Err(SchedulerError::NoAdapterAvailable(_))));
    }

    #[test]
    fn test_fallback_on_circuit_trip() {
        let mut sched = setup_scheduler();
        // Register second adapter for same model
        sched.register_adapter(
            "vllm-fallback".to_string(),
            vec!["qwen3-14b".to_string()],
            10,
            vec!["nvidia:h100:0".to_string()],
        );
        let req = make_request(RequestPriority::Normal, Some("qwen3-14b"));
        // Trip primary (5 failures for default threshold)
        for _ in 0..5 {
            sched.record_adapter_failure(&"ollama:0".to_string());
        }
        // Route should skip primary and pick fallback
        let selection = sched.route_request(&req).unwrap();
        assert_eq!(selection.adapter_id, "vllm-fallback");
    }

    #[test]
    fn test_health_monitor_dead_forces_open() {
        let mut sched = setup_scheduler();
        sched.set_adapter_health(&"ollama:0".to_string(), false);
        assert_eq!(
            sched.adapters["ollama:0"].circuit_breaker.state(),
            CircuitState::Open
        );
    }

    #[test]
    fn test_health_monitor_alive_resets() {
        let mut sched = setup_scheduler();
        for _ in 0..5 {
            sched.record_adapter_failure(&"ollama:0".to_string());
        }
        assert_eq!(
            sched.adapters["ollama:0"].circuit_breaker.state(),
            CircuitState::Open
        );
        sched.set_adapter_health(&"ollama:0".to_string(), true);
        assert_eq!(
            sched.adapters["ollama:0"].circuit_breaker.state(),
            CircuitState::Closed
        );
    }

    #[test]
    fn test_circuit_trip_emits_event() {
        let mut sched = setup_scheduler();
        for _ in 0..5 {
            sched.record_adapter_failure(&"ollama:0".to_string());
        }
        let events = sched.drain_events();
        assert!(!events.is_empty());
        assert!(matches!(
            &events[0],
            SchedulerEvent::CircuitTripped { adapter_id, .. } if adapter_id == "ollama:0"
        ));
    }

    #[test]
    fn test_drain_events_clears() {
        let mut sched = setup_scheduler();
        for _ in 0..5 {
            sched.record_adapter_failure(&"ollama:0".to_string());
        }
        let events = sched.drain_events();
        assert!(!events.is_empty());
        let events2 = sched.drain_events();
        assert!(events2.is_empty());
    }
}
