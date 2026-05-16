//! # MAI Rust SDK
//!
//! Typed Rust SDK for performance-critical L4 components that need direct
//! access to the MAI inference kernel API. Provides both blocking and async
//! clients with full type safety across the API boundary.
//!
//! Skeleton generated in Session 05; full client implementation in Session 11.
//! All types align with docs/api/openapi.yaml schemas.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ──────────────────────────────────────────────
// Error types (aligned with spec Section 6 error taxonomy)
// ──────────────────────────────────────────────

/// MAI API error types. Maps to OpenAPI ErrorResponse.error.type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaiErrorType {
    // Client errors (4xx)
    InvalidRequest,
    AuthenticationFailed,
    PermissionDenied,
    ModelUnavailable,
    ValidationError,
    RateLimited,
    ContextExceeded,
    // Server errors (5xx)
    InternalError,
    RequestFailed,
    Overloaded,
    AirGapViolation,
    PowerStateUnavailable,
    Timeout,
}

impl MaiErrorType {
    /// MAI-XYYY error code string (per spec Section 6 taxonomy)
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "MAI-4001",
            Self::AuthenticationFailed => "MAI-4003",
            Self::PermissionDenied => "MAI-4005",
            Self::ModelUnavailable => "MAI-4004",
            Self::ValidationError => "MAI-4006",
            Self::RateLimited => "MAI-4007",
            Self::ContextExceeded => "MAI-4008",
            Self::InternalError => "MAI-5001",
            Self::RequestFailed => "MAI-5002",
            Self::Overloaded => "MAI-5003",
            Self::AirGapViolation => "MAI-5004",
            Self::PowerStateUnavailable => "MAI-5005",
            Self::Timeout => "MAI-5006",
        }
    }

    /// Whether this error type is retryable (spec Section 6.2)
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::Overloaded
                | Self::Timeout
                | Self::PowerStateUnavailable
        )
    }
}

/// Error returned by the MAI API. Maps to OpenAPI ErrorResponse schema.
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
#[error("MAI API error {code}: {message}")]
pub struct MaiError {
    /// Error type classification
    #[serde(rename = "type")]
    pub error_type: MaiErrorType,
    /// MAI-XYYY error code
    pub code: String,
    /// Human-readable message (never contains backend internals)
    pub message: String,
    /// Retry hint in seconds (present for retryable errors)
    pub retry_after_seconds: Option<u64>,
    /// Request ID for correlation
    pub request_id: Option<String>,
}

impl MaiError {
    pub fn is_retryable(&self) -> bool {
        self.error_type.is_retryable()
    }
}

/// SDK-level errors (transport, serialization, config)
#[derive(Error, Debug)]
pub enum SdkError {
    #[error("API error: {0}")]
    Api(MaiError),

    #[error("Connection failed: {0}")]
    Connection(String),

    #[error("Request serialization failed: {0}")]
    Serialization(String),

    #[error("Response deserialization failed: {0}")]
    Deserialization(String),

    #[error("Request timed out after {0:?}")]
    Timeout(Duration),

    #[error("Invalid configuration: {0}")]
    Config(String),
}

pub type SdkResult<T> = Result<T, SdkError>;

// ──────────────────────────────────────────────
// Enums (aligned with OpenAPI enum definitions)
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for RequestPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Maps to OpenAPI ChatChoice.finish_reason and adapter::FinishReason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    MaxTokens,
    StopSequence,
    ToolCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRole {
    Admin,
    Adult,
    Teen,
    Child,
    Guest,
}

/// Maps to OpenAPI ProfileObject.content_safety.filter_level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentFilterLevel {
    None,
    Moderate,
    Strict,
}

/// Maps to OpenAPI AdapterHealthEntry.status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Maps to OpenAPI GpuHealthEntry / HardwareHealthResponse thermal state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalState {
    Normal,
    Elevated,
    Throttled,
    Critical,
}

/// Maps to OpenAPI HardwareHealthResponse.network_state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    AirGapCompliant,
    Connected,
    NonCompliant,
}

/// Maps to OpenAPI PowerStateResponse.state and power::PowerState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    Off,
    DeepVaultSleep,
    Sentinel,
    FullInference,
    ThermalThrottle,
}

/// Maps to OpenAPI ModelObject.status and registry::ModelStatus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    ColdStorage,
    Loading,
    Loaded,
    Active,
    Evicting,
    Evicted,
}

/// Maps to OpenAPI ModelObject.format and registry::ModelFormat
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ModelFormat {
    Gguf,
    SafeTensors,
    Exl2,
    Gptq,
}

/// Maps to OpenAPI HealthResponse.status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

// ──────────────────────────────────────────────
// Request types (aligned with OpenAPI request schemas)
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub input: Vec<String>,
}

/// B6 FIX: Uses `prompt` field (not `messages`) per OpenAPI StructuredRequest schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredRequest {
    pub model: String,
    pub prompt: String,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub functions: Vec<FunctionDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerTransitionRequest {
    pub target_state: PowerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ──────────────────────────────────────────────
// Response types (aligned with OpenAPI response schemas)
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub text: String,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: Usage,
}

/// B5 FIX: Added input_tokens field per OpenAPI EmbeddingData schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingData {
    pub object: String,
    pub embedding: Vec<f32>,
    pub index: u32,
    /// IM extension: per-embedding token count (maps to adapter::Embedding.input_tokens)
    pub input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub object: String,
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub output: serde_json::Value,
    pub usage: Usage,
    pub schema_valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallData {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub function_call: FunctionCallData,
    pub usage: Usage,
}

// ──────────────────────────────────────────────
// Model types (aligned with OpenAPI ModelObject/CapabilityInfo schemas)
// ──────────────────────────────────────────────

/// N1 FIX: Matches OpenAPI CapabilityInfo schema exactly
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInfo {
    pub chat: bool,
    pub completion: bool,
    pub embedding: bool,
    pub vision: bool,
    pub structured_output: bool,
    pub max_context_tokens: u32,
    #[serde(default)]
    pub supported_languages: Vec<String>,
}

/// Matches OpenAPI ModelObject schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelObject {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
    pub name: String,
    pub version: String,
    pub format: ModelFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
    pub size_bytes: u64,
    pub required_vram_bytes: u64,
    pub status: ModelStatus,
    pub capabilities: CapabilityInfo,
    #[serde(default)]
    pub compatible_backends: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<ModelSecurity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSecurity {
    pub signature_algorithm: String,
    pub integrity_verified: bool,
}

/// Matches OpenAPI ModelDetail schema (allOf ModelObject + extra fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetail {
    #[serde(flatten)]
    pub base: ModelObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_assignment: Option<AdapterAssignment>,
    pub vram_allocated_bytes: u64,
    pub request_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterAssignment {
    pub adapter_id: String,
    pub gpu_id: String,
}

// ──────────────────────────────────────────────
// Health types (N2 FIX: aligned with OpenAPI HealthResponse schema)
// ──────────────────────────────────────────────

/// Matches OpenAPI HealthResponse schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: SystemHealthStatus,
    pub air_gap_verified: bool,
    pub power_state: PowerState,
    pub uptime_seconds: u64,
    pub adapters: AdapterSummary,
    pub hardware: HardwareSummary,
    pub system: SystemSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterSummary {
    pub total: u32,
    pub healthy: u32,
    pub degraded: u32,
    pub unhealthy: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareSummary {
    pub gpus: u32,
    pub total_vram_bytes: u64,
    pub used_vram_bytes: u64,
    pub thermal_state: ThermalState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSummary {
    pub cpu_load_percent: f32,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub disk_vault_free_bytes: u64,
}

/// Matches OpenAPI AdapterHealthEntry schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterHealthEntry {
    pub status: AdapterStatus,
    pub last_heartbeat: String,
    pub missed_heartbeats: u32,
    pub avg_latency_ms: f64,
    pub error_rate_5min: f64,
    pub vram_usage_bytes: u64,
    pub active_requests: u32,
}

/// Matches OpenAPI AdapterHealthMap schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterHealthMap {
    pub adapters: HashMap<String, AdapterHealthEntry>,
}

/// Matches OpenAPI GpuHealthEntry schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuHealthEntry {
    pub temperature_celsius: f32,
    pub fan_speed_percent: u32,
    pub vram_used_bytes: u64,
    pub vram_total_bytes: u64,
    pub power_limit_watts: u32,
    pub compute_utilization_percent: u32,
}

/// Matches OpenAPI HardwareHealthResponse schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareHealthResponse {
    pub gpus: HashMap<String, GpuHealthEntry>,
    pub power_draw_watts: f32,
    pub thermal_state: ThermalState,
    pub network_state: NetworkState,
}

// ──────────────────────────────────────────────
// Power types (aligned with OpenAPI PowerStateResponse schema)
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDemotion {
    pub enabled: bool,
    /// Minutes remaining before auto-demotion (None if disabled or no countdown)
    pub idle_minutes_remaining: Option<u32>,
    /// Target state for auto-demotion (None if disabled)
    pub next_state: Option<PowerState>,
}

/// Matches OpenAPI PowerStateResponse schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerStateResponse {
    pub state: PowerState,
    pub estimated_power_watts: f32,
    pub auto_demotion: AutoDemotion,
    pub promotion_available: bool,
    pub promotion_latency_target_ms: u32,
}

/// Matches OpenAPI PowerTransitionResponse schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerTransitionResponse {
    pub transition_id: String,
    pub from: PowerState,
    pub to: PowerState,
    pub status: String,
    pub estimated_latency_ms: u32,
}

// ──────────────────────────────────────────────
// Profile types (N4 FIX: aligned with OpenAPI ProfileObject schema)
// ──────────────────────────────────────────────

/// Matches OpenAPI ProfileObject.content_safety
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSafety {
    pub enabled: bool,
    pub filter_level: ContentFilterLevel,
}

/// Matches OpenAPI ProfileObject.rate_limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimits {
    pub requests_per_minute: Option<u32>,
    pub tokens_per_hour: Option<u64>,
}

/// Matches OpenAPI ProfilePermissions schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePermissions {
    /// Model ID patterns. ["*"] = all models.
    pub model_access: Vec<String>,
    pub max_context_tokens: Option<u32>,
    /// Endpoint patterns. ["*"] = all endpoints.
    pub allowed_endpoints: Vec<String>,
    pub can_manage_models: bool,
    pub can_manage_power: bool,
    pub can_view_audit: bool,
    pub can_manage_profiles: bool,
}

/// Matches OpenAPI ProfileObject schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileObject {
    pub profile_id: String,
    pub name: String,
    pub role: ProfileRole,
    pub created_at: String,
    pub permissions: ProfilePermissions,
    pub priority: RequestPriority,
    pub rate_limits: RateLimits,
    pub content_safety: ContentSafety,
}

// ──────────────────────────────────────────────
// Audit types (N3 FIX: aligned with OpenAPI AuditEntry schema)
// ──────────────────────────────────────────────

/// Matches OpenAPI AuditEntry schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub request_id: String,
    pub profile_id: String,
    pub endpoint: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub latency_ms: u32,
    pub status_code: u16,
    pub priority: String,
    pub hash: String,
    pub prev_hash: String,
}

/// Matches OpenAPI AuditLogResponse schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub total_entries: u64,
    pub offset: u32,
    pub limit: u32,
    pub entries: Vec<AuditEntry>,
}

// ──────────────────────────────────────────────
// Client configuration
// ──────────────────────────────────────────────

/// Client configuration for connecting to the MAI API
/// B3 FIX: Default port is 8420 (matching spec, OpenAPI, and Python SDK)
#[derive(Debug, Clone)]
pub struct MaiClientConfig {
    /// Base URL (default: http://localhost:8420)
    pub base_url: String,
    /// Profile ID for authentication (X-IM-Profile header)
    pub profile_id: String,
    /// Default request priority (X-IM-Priority header)
    pub priority: RequestPriority,
    /// Request timeout
    pub timeout: Duration,
    /// Custom headers
    pub extra_headers: HashMap<String, String>,
}

impl Default for MaiClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8420".to_string(),
            profile_id: String::new(),
            priority: RequestPriority::Normal,
            timeout: Duration::from_secs(30),
            extra_headers: HashMap::new(),
        }
    }
}

/// Async MAI API client
///
/// Full implementation in Session 11. This skeleton defines the public
/// interface that L4 components can program against.
pub struct MaiClient {
    config: MaiClientConfig,
}

impl MaiClient {
    /// Create a new client with the given configuration
    pub fn new(config: MaiClientConfig) -> SdkResult<Self> {
        if config.profile_id.is_empty() {
            return Err(SdkError::Config("profile_id is required".to_string()));
        }
        Ok(Self { config })
    }

    // ── Inference endpoints ──

    /// POST /v1/chat/completions (OpenAI-compatible)
    pub async fn chat(&self, _request: ChatCompletionRequest) -> SdkResult<ChatCompletionResponse> {
        todo!("Session 11: HTTP client")
    }

    /// POST /v1/chat/completions with stream=true
    /// Returns an async stream of SSE chunks. Resume via Last-Event-ID.
    pub async fn chat_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> SdkResult<ChatStreamHandle> {
        todo!("Session 11: SSE streaming")
    }

    /// POST /v1/completions
    pub async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> SdkResult<CompletionResponse> {
        todo!("Session 11: HTTP client")
    }

    /// POST /v1/embeddings
    pub async fn embed(&self, _request: EmbeddingRequest) -> SdkResult<EmbeddingResponse> {
        todo!("Session 11: HTTP client")
    }

    /// POST /v1/generate/structured
    pub async fn structured(
        &self,
        _request: StructuredRequest,
    ) -> SdkResult<StructuredResponse> {
        todo!("Session 11: HTTP client")
    }

    /// POST /v1/generate/function_call
    pub async fn function_call(
        &self,
        _request: FunctionCallRequest,
    ) -> SdkResult<FunctionCallResponse> {
        todo!("Session 11: HTTP client")
    }

    // ── Model management endpoints ──

    /// GET /v1/models
    pub async fn list_models(&self) -> SdkResult<Vec<ModelObject>> {
        todo!("Session 11: HTTP client")
    }

    /// GET /v1/models/{id}
    pub async fn get_model(&self, _model_id: &str) -> SdkResult<ModelDetail> {
        todo!("Session 11: HTTP client")
    }

    // ── Health endpoints ──

    /// GET /v1/health
    pub async fn health(&self) -> SdkResult<HealthResponse> {
        todo!("Session 11: HTTP client")
    }

    /// GET /v1/health/adapters
    pub async fn adapter_health(&self) -> SdkResult<AdapterHealthMap> {
        todo!("Session 11: HTTP client")
    }

    /// GET /v1/health/hardware
    pub async fn hardware_health(&self) -> SdkResult<HardwareHealthResponse> {
        todo!("Session 11: HTTP client")
    }

    // ── Power management endpoints ──

    /// GET /v1/power/state
    pub async fn power_state(&self) -> SdkResult<PowerStateResponse> {
        todo!("Session 11: HTTP client")
    }

    /// POST /v1/power/transition
    pub async fn transition_power(
        &self,
        _request: PowerTransitionRequest,
    ) -> SdkResult<PowerTransitionResponse> {
        todo!("Session 11: HTTP client")
    }

    // ── Profile endpoints ──

    /// GET /v1/profiles/me
    pub async fn get_profile(&self) -> SdkResult<ProfileObject> {
        todo!("Session 11: HTTP client")
    }

    // ── Audit endpoints ──

    /// GET /v1/audit/log
    pub async fn audit_log(
        &self,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> SdkResult<AuditLogResponse> {
        todo!("Session 11: HTTP client")
    }

    /// Access to the client configuration
    pub fn config(&self) -> &MaiClientConfig {
        &self.config
    }
}

/// Handle for a streaming chat completion response
///
/// Wraps an SSE stream with resume capability via Last-Event-ID.
/// Full implementation in Session 11.
pub struct ChatStreamHandle {
    _private: (),
}

impl ChatStreamHandle {
    /// Get the next chunk from the stream
    pub async fn next_chunk(&mut self) -> SdkResult<Option<ChatCompletionChunk>> {
        todo!("Session 11: SSE stream")
    }

    /// Get the last event ID for resume
    pub fn last_event_id(&self) -> Option<&str> {
        todo!("Session 11: resume protocol")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_spec_taxonomy() {
        // Spec Section 6 error code table
        assert_eq!(MaiErrorType::InvalidRequest.code(), "MAI-4001");
        assert_eq!(MaiErrorType::AuthenticationFailed.code(), "MAI-4003");
        assert_eq!(MaiErrorType::ModelUnavailable.code(), "MAI-4004");
        assert_eq!(MaiErrorType::PermissionDenied.code(), "MAI-4005");
        assert_eq!(MaiErrorType::ValidationError.code(), "MAI-4006");
        assert_eq!(MaiErrorType::RateLimited.code(), "MAI-4007");
        assert_eq!(MaiErrorType::ContextExceeded.code(), "MAI-4008");
        assert_eq!(MaiErrorType::InternalError.code(), "MAI-5001");
        assert_eq!(MaiErrorType::RequestFailed.code(), "MAI-5002");
        assert_eq!(MaiErrorType::Overloaded.code(), "MAI-5003");
        assert_eq!(MaiErrorType::AirGapViolation.code(), "MAI-5004");
        assert_eq!(MaiErrorType::PowerStateUnavailable.code(), "MAI-5005");
        assert_eq!(MaiErrorType::Timeout.code(), "MAI-5006");
    }

    #[test]
    fn retryable_errors_identified() {
        assert!(MaiErrorType::RateLimited.is_retryable());
        assert!(MaiErrorType::Overloaded.is_retryable());
        assert!(MaiErrorType::Timeout.is_retryable());
        assert!(MaiErrorType::PowerStateUnavailable.is_retryable());
        // Non-retryable
        assert!(!MaiErrorType::InvalidRequest.is_retryable());
        assert!(!MaiErrorType::AirGapViolation.is_retryable());
        assert!(!MaiErrorType::InternalError.is_retryable());
    }

    #[test]
    fn default_config_is_port_8420() {
        let config = MaiClientConfig::default();
        assert_eq!(config.base_url, "http://localhost:8420");
        assert_eq!(config.priority, RequestPriority::Normal);
    }

    #[test]
    fn client_requires_profile_id() {
        let config = MaiClientConfig::default();
        assert!(MaiClient::new(config).is_err());
    }

    #[test]
    fn finish_reason_has_no_content_filter() {
        // B1 FIX verification: FinishReason only has spec-defined variants
        let json = r#""tool_calls""#;
        let reason: FinishReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, FinishReason::ToolCalls);

        // content_filter should NOT deserialize (variant removed)
        let bad = r#""content_filter""#;
        assert!(serde_json::from_str::<FinishReason>(bad).is_err());
    }

    #[test]
    fn content_filter_level_matches_openapi() {
        // B2 FIX verification: uses none/moderate/strict per OpenAPI
        let json = r#""none""#;
        let level: ContentFilterLevel = serde_json::from_str(json).unwrap();
        assert_eq!(level, ContentFilterLevel::None);

        let json = r#""strict""#;
        let level: ContentFilterLevel = serde_json::from_str(json).unwrap();
        assert_eq!(level, ContentFilterLevel::Strict);
    }
}
