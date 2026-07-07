//! API request/response types for the MAI REST and gRPC interfaces.
//!
//! These types implement the OpenAI-compatible API surface defined in
//! MAI-API-SURFACE-SPEC.md. All types derive Serialize, Deserialize,
//! Debug, Clone for wire compatibility and logging.
//!
//! # Type Alignment
//!
//! API types map to mai-core internal types via `From` conversions.
//! The API layer never exposes internal type names or structures.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};
const MAX_EMBEDDING_BATCH_ITEMS: usize = 256;
const MAX_EMBEDDING_ITEM_CHARS: usize = 32_768;

fn validate_embedding_input(input: &EmbeddingInput) -> Result<(), ValidationError> {
    match input {
        EmbeddingInput::Single(s) => {
            if s.trim().is_empty() {
                return Err(ValidationError::new("empty"));
            }
            if s.chars().count() > MAX_EMBEDDING_ITEM_CHARS {
                return Err(ValidationError::new("too_large"));
            }
        }
        EmbeddingInput::Batch(v) => {
            if v.is_empty() {
                return Err(ValidationError::new("empty"));
            }
            if v.len() > MAX_EMBEDDING_BATCH_ITEMS {
                return Err(ValidationError::new("too_many_items"));
            }
            for s in v {
                if s.trim().is_empty() {
                    return Err(ValidationError::new("empty_item"));
                }
                if s.chars().count() > MAX_EMBEDDING_ITEM_CHARS {
                    return Err(ValidationError::new("too_large"));
                }
            }
        }
    }
    Ok(())
}

fn validate_tool_type_function(value: &str) -> Result<(), ValidationError> {
    if value == "function" {
        Ok(())
    } else {
        Err(ValidationError::new("unsupported_tool_type"))
    }
}

// ─── Chat Completion Request ────────────────────────────────────────

/// OpenAI-compatible chat completion request
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ChatCompletionRequest {
    /// Model identifier (optional; scheduler picks if omitted)
    #[serde(default)]
    pub model: Option<String>,
    /// Conversation messages
    #[validate(length(min = 1, message = "messages must be non-empty"))]
    pub messages: Vec<ApiChatMessage>,
    /// Whether to stream response tokens via SSE
    #[serde(default)]
    pub stream: bool,
    /// Sampling temperature (0.0 - 2.0)
    #[serde(default)]
    #[validate(range(min = 0.0, max = 2.0, message = "temperature must be in [0.0, 2.0]"))]
    pub temperature: Option<f32>,
    /// Nucleus sampling threshold
    #[serde(default)]
    #[validate(range(min = 0.0, max = 1.0, message = "top_p must be in [0.0, 1.0]"))]
    pub top_p: Option<f32>,
    /// Maximum tokens to generate
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Stop sequences
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    /// Frequency penalty (-2.0 to 2.0)
    #[serde(default)]
    #[validate(range(
        min = -2.0,
        max = 2.0,
        message = "frequency_penalty must be in [-2.0, 2.0]"
    ))]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0)
    #[serde(default)]
    #[validate(range(
        min = -2.0,
        max = 2.0,
        message = "presence_penalty must be in [-2.0, 2.0]"
    ))]
    pub presence_penalty: Option<f32>,
    /// User identifier for abuse tracking (local-only)
    #[serde(default)]
    pub user: Option<String>,
}

/// Single chat message in API format
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ApiChatMessage {
    /// Role: system, user, assistant, tool
    #[validate(length(min = 1, message = "role must be non-empty"))]
    pub role: String,
    /// Message content
    #[validate(length(min = 1, message = "content must be non-empty"))]
    pub content: String,
    /// Optional name for multi-participant chats
    #[serde(default)]
    pub name: Option<String>,
}

// ─── Embedding Request ──────────────────────────────────────────────

/// OpenAI-compatible embedding request
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct EmbeddingRequest {
    /// Model to use for embeddings
    #[serde(default)]
    pub model: Option<String>,
    /// Input text(s) to embed
    #[validate(custom(function = "validate_embedding_input"))]
    pub input: EmbeddingInput,
    /// Encoding format (float or base64)
    #[serde(default = "default_encoding_format")]
    #[validate(custom(function = "validate_encoding_format"))]
    pub encoding_format: String,
}

/// Embedding input can be a single string or array of strings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Batch(Vec<String>),
}

fn default_encoding_format() -> String {
    "float".to_string()
}

fn validate_encoding_format(value: &str) -> Result<(), ValidationError> {
    match value {
        "float" | "base64" => Ok(()),
        _ => {
            let mut err = ValidationError::new("encoding_format");
            err.message = Some("encoding_format must be 'float' or 'base64'".into());
            Err(err)
        }
    }
}

// ─── Structured Generation Request ──────────────────────────────────

/// Request for structured/constrained output generation
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct StructuredGenerationRequest {
    /// Model identifier
    #[serde(default)]
    pub model: Option<String>,
    /// Conversation messages providing context
    pub messages: Vec<ApiChatMessage>,
    /// JSON schema the output must conform to
    pub response_format: ResponseFormat,
    /// Sampling temperature
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Maximum tokens
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Response format specification for structured output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFormat {
    /// Format type: "json_object" or "json_schema"
    #[serde(rename = "type")]
    pub format_type: String,
    /// JSON schema definition (when type is "json_schema")
    #[serde(default)]
    pub json_schema: Option<serde_json::Value>,
}

// ─── Function Call Request ──────────────────────────────────────────

/// Request with tool/function calling capability
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct FunctionCallRequest {
    /// Model identifier
    #[serde(default)]
    pub model: Option<String>,
    /// Conversation messages
    #[validate(length(min = 1, message = "messages must be non-empty"))]
    pub messages: Vec<ApiChatMessage>,
    /// Available tools/functions
    #[validate(length(min = 1, message = "tools must be non-empty"))]
    pub tools: Vec<ToolDefinition>,
    /// Tool choice strategy: "auto", "none", or specific tool
    #[serde(default = "default_tool_choice")]
    pub tool_choice: String,
    /// Sampling temperature
    #[serde(default)]
    #[validate(range(min = 0.0, max = 2.0, message = "temperature must be in [0.0, 2.0]"))]
    pub temperature: Option<f32>,
    /// Maximum tokens
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Tool/function definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ToolDefinition {
    /// Tool type (currently always "function")
    #[serde(rename = "type")]
    #[validate(custom(function = "validate_tool_type_function"))]
    pub tool_type: String,
    /// Function definition
    #[validate(nested)]
    pub function: FunctionDefinition,
}

/// Function definition within a tool
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct FunctionDefinition {
    /// Function name
    #[validate(length(min = 1, message = "function.name must be non-empty"))]
    pub name: String,
    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for function parameters
    pub parameters: serde_json::Value,
}

fn default_tool_choice() -> String {
    "auto".to_string()
}

// ─── Chat Completion Response ───────────────────────────────────────

/// OpenAI-compatible chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// Response identifier
    pub id: String,
    /// Object type (always "chat.completion")
    pub object: String,
    /// Creation timestamp (unix epoch seconds)
    pub created: u64,
    /// Model that generated the response
    pub model: String,
    /// Response choices (typically 1)
    pub choices: Vec<ChatChoice>,
    /// Token usage statistics
    pub usage: TokenUsage,
}

/// Single response choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    /// Choice index
    pub index: u32,
    /// Generated message
    pub message: ApiChatMessage,
    /// Why generation stopped: "stop", "length", "tool_calls"
    pub finish_reason: Option<String>,
}

/// Token usage counters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens in the prompt
    pub prompt_tokens: u32,
    /// Tokens generated
    pub completion_tokens: u32,
    /// Total tokens consumed
    pub total_tokens: u32,
}

// ─── SSE Streaming Chunk ────────────────────────────────────────────

/// SSE streaming chunk for chat completions (stream=true)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    /// Chunk identifier (same as parent response id)
    pub id: String,
    /// Object type (always "chat.completion.chunk")
    pub object: String,
    /// Creation timestamp
    pub created: u64,
    /// Model name
    pub model: String,
    /// Delta choices
    pub choices: Vec<ChunkChoice>,
}

/// Streaming delta choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// Choice index
    pub index: u32,
    /// Incremental content delta
    pub delta: ChunkDelta,
    /// Finish reason (only present on final chunk)
    pub finish_reason: Option<String>,
}

/// Content delta in a streaming chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkDelta {
    /// Role (only present in first chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Content fragment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// ─── Embedding Response ─────────────────────────────────────────────

/// OpenAI-compatible embedding response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// Object type
    pub object: String,
    /// Embedding results
    pub data: Vec<EmbeddingData>,
    /// Model used
    pub model: String,
    /// Token usage
    pub usage: EmbeddingUsage,
}

/// Single embedding result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingData {
    /// Object type
    pub object: String,
    /// Embedding vector
    pub embedding: Vec<f32>,
    /// Index in input batch
    pub index: u32,
}

/// Embedding token usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingUsage {
    /// Tokens in input
    pub prompt_tokens: u32,
    /// Total tokens
    pub total_tokens: u32,
}

// ─── MAI-Specific Types ─────────────────────────────────────────────

/// Model listing response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    /// Object type
    pub object: String,
    /// Available models
    pub data: Vec<ModelDetail>,
}

/// Detailed model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetail {
    /// Model identifier
    pub id: String,
    /// Object type
    pub object: String,
    /// Creation timestamp
    pub created: u64,
    /// Owner (always "island-mountain")
    pub owned_by: String,
    /// Model capabilities
    pub capabilities: ModelCapabilities,
    /// Current lifecycle status
    pub status: String,
    /// Size on disk in bytes
    pub size_bytes: u64,
    /// VRAM required in bytes
    pub required_vram_bytes: u64,
}

/// Model capability flags exposed via API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub chat: bool,
    pub completion: bool,
    pub embedding: bool,
    pub vision: bool,
    pub structured_output: bool,
    pub max_context_tokens: u32,
}

/// Aggregate health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Overall system status: "healthy", "degraded", "unhealthy"
    pub status: String,
    /// Alert level
    pub alert_level: String,
    /// Per-adapter health summaries
    pub adapters: Vec<AdapterHealthSummary>,
    /// Hardware health summary
    pub hardware: HardwareHealthSummary,
    /// System resource summary
    pub system: SystemHealthSummary,
}

/// Adapter health for API consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterHealthSummary {
    /// Adapter identifier
    pub id: String,
    /// Status: "healthy", "degraded", "unhealthy", "unknown"
    pub status: String,
    /// Average latency in ms
    pub avg_latency_ms: f64,
    /// Error rate (0.0 - 1.0)
    pub error_rate: f32,
}

/// Hardware health for API consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareHealthSummary {
    /// Per-GPU summaries
    pub gpus: Vec<GpuHealthSummary>,
    /// Air-gap compliance status
    pub air_gap_status: String,
}

/// GPU health for API consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuHealthSummary {
    /// Device identifier
    pub device_id: String,
    /// Temperature in Celsius
    pub temperature_celsius: f32,
    /// VRAM utilization percentage
    pub vram_utilization_percent: f32,
    /// Power draw in watts
    pub power_watts: u32,
    /// Thermal state
    pub thermal_state: String,
}

/// System resource health for API consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealthSummary {
    /// Disk utilization percentage
    pub disk_utilization_percent: f32,
    /// RAM utilization percentage
    pub ram_utilization_percent: f32,
    /// CPU utilization percentage
    pub cpu_utilization_percent: f32,
}

/// Power state response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerStateResponse {
    /// Current power state
    pub state: String,
    /// Estimated power draw in watts
    pub estimated_watts: u32,
    /// Time in current state (seconds)
    pub state_duration_secs: u64,
    /// Whether auto-demotion is pending
    pub demotion_pending: bool,
}

/// Power state transition request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerTransitionRequest {
    /// Target state or trigger action
    pub action: String,
    /// Optional reason for audit trail
    #[serde(default)]
    pub reason: Option<String>,
}

/// Registry query response (model listing with status)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryQueryResponse {
    /// Total models registered
    pub total_models: usize,
    /// Models currently loaded
    pub loaded_models: usize,
    /// Model entries
    pub models: Vec<ModelDetail>,
}

/// Audit log response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogResponse {
    /// Audit entries (newest first)
    pub entries: Vec<AuditLogEntry>,
    /// Total entries available
    pub total: u64,
    /// Current page offset
    pub offset: u64,
    /// Page size
    pub limit: u64,
}

/// Single audit entry for API consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Entry sequence number
    pub sequence: u64,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Profile that made the request
    pub profile_id: String,
    /// HTTP method
    pub method: String,
    /// Request path
    pub endpoint: String,
    /// Model used (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Input token count
    pub tokens_in: u32,
    /// Output token count
    pub tokens_out: u32,
    /// Response latency in ms
    pub latency_ms: u64,
    /// HTTP status code
    pub status_code: u16,
    /// Request identifier
    pub request_id: String,
    /// Hash chain integrity value
    pub chain_hash: String,
}

// ─── Profile Wire Types (API responses) ────────────────────────────

/// Profile summary for API responses (wire format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResponse {
    /// Profile identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Role string
    pub role: String,
}

/// Profile list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileListResponse {
    pub profiles: Vec<ProfileResponse>,
}

// ─── Adapter Wire Types (API responses) ─────────────────────────────

/// Adapter listing response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterListResponse {
    pub adapters: Vec<AdapterHealthSummary>,
}

// ─── Model Install/Discover/Remove ──────────────────────────────────

/// Response for model install operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInstallResponse {
    pub model_id: String,
    pub status: String,
    pub integrity_verified: bool,
    pub signature_verified: bool,
    pub message: String,
}

/// USB package discovery response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverResponse {
    pub packages: Vec<DiscoveredPackage>,
    pub drives_scanned: usize,
    pub errors: Vec<String>,
}

/// A single discovered package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPackage {
    pub name: String,
    pub model_name: String,
    pub version: String,
    pub format: String,
    pub size_bytes: u64,
    pub model_id: String,
}

/// Response for model remove operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRemoveResponse {
    pub model_id: String,
    pub status: String,
    /// V7: the model's encryption key was retired (cryptographic erasure) —
    /// the honest deletion guarantee on copy-on-write storage. Replaces the
    /// former `secure_wipe` overwrite claim.
    pub crypto_erased: bool,
    pub snapshot_created: bool,
    pub message: String,
}

// ─── Model Load/Unload Response ─────────────────────────────────────

/// Response for model load/unload operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOperationResponse {
    /// Operation performed
    pub operation: String,
    /// Model affected
    pub model_id: String,
    /// Result status
    pub status: String,
    /// Human-readable message
    pub message: String,
}

/// Response for model benchmark operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBenchmarkResponse {
    pub model_id: String,
    pub tokens_per_sec: f64,
    pub ttft_ms: u64,
    pub memory_used_bytes: u64,
    pub completed_at_epoch: u64,
}

impl From<mai_core::models::BenchmarkResult> for ModelBenchmarkResponse {
    fn from(result: mai_core::models::BenchmarkResult) -> Self {
        Self {
            model_id: result.model_id,
            tokens_per_sec: result.tokens_per_sec,
            ttft_ms: result.ttft_ms,
            memory_used_bytes: result.memory_used_bytes,
            completed_at_epoch: result.completed_at_epoch,
        }
    }
}

/// Update check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResponse {
    pub available: Vec<UpdateModelInfo>,
    pub current: Vec<String>,
}

/// Available update model info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateModelInfo {
    pub name: String,
    pub version: String,
    pub size: u64,
    pub url: String,
    pub tier: String,
}

impl From<mai_core::models::UpdateModel> for UpdateModelInfo {
    fn from(model: mai_core::models::UpdateModel) -> Self {
        Self {
            name: model.name,
            version: model.version,
            size: model.size,
            url: model.url,
            tier: model.tier.as_str().to_string(),
        }
    }
}

/// Start background update download request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDownloadRequest {
    pub name: String,
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub auto_install: bool,
}

/// Start background update download response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDownloadResponse {
    pub download_id: String,
    pub status: String,
    pub message: String,
}

/// Update download status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatusResponse {
    pub downloads: Vec<UpdateDownloadStatus>,
}

/// Single background update status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDownloadStatus {
    pub download_id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub progress_percent: u8,
    pub bytes_downloaded: u64,
    pub message: String,
}

// ─── Registry Scan Response ─────────────────────────────────────────

/// Response for registry scan operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryScanResponse {
    /// Number of models discovered
    pub models_found: usize,
    /// Number of new models added
    pub new_models: usize,
    /// Scan status message
    pub message: String,
}
// ─── Profile Types ──────────────────────────────────────────────────

/// Family profile information extracted from request headers.
/// This is the internal representation used by middleware and handlers.
/// The wire format for API responses uses separate types.
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    /// Profile unique identifier
    pub profile_id: String,
    /// Access role
    pub role: ProfileRole,
    /// Optional display name
    pub display_name: Option<String>,
    /// Computed permissions from role
    pub permissions: ProfilePermissions,
}

/// Profile access roles with hierarchical permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProfileRole {
    /// Full system access including power control, registry, audit
    Admin,
    /// Inference, model listing, own profile
    Adult,
    /// Inference with filtered models only, own profile
    Teen,
    /// Inference with child-safe models and content filtering, own profile
    Child,
    /// Inference with default model only, health check only
    Guest,
}

/// Permissions derived from a profile role
#[derive(Debug, Clone)]
pub struct ProfilePermissions {
    /// Can perform inference requests
    pub can_inference: bool,
    /// Can list and view model details
    pub can_list_models: bool,
    /// Can load/unload models
    pub can_manage_models: bool,
    /// Can control power state
    pub can_power_control: bool,
    /// Can modify registry entries
    pub can_registry_write: bool,
    /// Can view audit logs
    pub can_view_audit: bool,
    /// Can manage profiles
    pub can_manage_profiles: bool,
    /// Model access filter (None = all models)
    pub model_filter: Option<ModelAccessFilter>,
    /// Content filtering level
    pub content_filter: ContentFilterLevel,
}

/// Model access restriction for Teen/Child profiles
#[derive(Debug, Clone)]
pub enum ModelAccessFilter {
    /// Only models explicitly tagged as teen-appropriate
    TeenSafe,
    /// Only models explicitly tagged as child-safe
    ChildSafe,
    /// Only the system default model
    DefaultOnly,
}

/// Content filtering strictness level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentFilterLevel {
    /// No filtering (Admin, Adult)
    None,
    /// Moderate filtering (Teen)
    Moderate,
    /// Strict filtering (Child)
    Strict,
}

impl ProfileRole {
    /// Derive permissions from role
    pub fn permissions(self) -> ProfilePermissions {
        match self {
            Self::Admin => ProfilePermissions {
                can_inference: true,
                can_list_models: true,
                can_manage_models: true,
                can_power_control: true,
                can_registry_write: true,
                can_view_audit: true,
                can_manage_profiles: true,
                model_filter: None,
                content_filter: ContentFilterLevel::None,
            },
            Self::Adult => ProfilePermissions {
                can_inference: true,
                can_list_models: true,
                can_manage_models: false,
                can_power_control: false,
                can_registry_write: false,
                can_view_audit: false,
                can_manage_profiles: false,
                model_filter: None,
                content_filter: ContentFilterLevel::None,
            },
            Self::Teen => ProfilePermissions {
                can_inference: true,
                can_list_models: true,
                can_manage_models: false,
                can_power_control: false,
                can_registry_write: false,
                can_view_audit: false,
                can_manage_profiles: false,
                model_filter: Some(ModelAccessFilter::TeenSafe),
                content_filter: ContentFilterLevel::Moderate,
            },
            Self::Child => ProfilePermissions {
                can_inference: true,
                can_list_models: false,
                can_manage_models: false,
                can_power_control: false,
                can_registry_write: false,
                can_view_audit: false,
                can_manage_profiles: false,
                model_filter: Some(ModelAccessFilter::ChildSafe),
                content_filter: ContentFilterLevel::Strict,
            },
            Self::Guest => ProfilePermissions {
                can_inference: true,
                can_list_models: false,
                can_manage_models: false,
                can_power_control: false,
                can_registry_write: false,
                can_view_audit: false,
                can_manage_profiles: false,
                model_filter: Some(ModelAccessFilter::DefaultOnly),
                content_filter: ContentFilterLevel::None,
            },
        }
    }
}

// ─── From Conversions: API -> mai-core ──────────────────────────────

impl From<&ApiChatMessage> for mai_core::scheduler::ChatMessage {
    fn from(msg: &ApiChatMessage) -> Self {
        Self {
            role: msg.role.clone(),
            content: msg.content.clone(),
        }
    }
}

impl From<&ChatCompletionRequest> for mai_core::scheduler::RequestPayload {
    fn from(req: &ChatCompletionRequest) -> Self {
        Self::Chat {
            messages: req.messages.iter().map(Into::into).collect(),
        }
    }
}

impl From<&EmbeddingRequest> for mai_core::scheduler::RequestPayload {
    fn from(req: &EmbeddingRequest) -> Self {
        let texts = match &req.input {
            EmbeddingInput::Single(s) => vec![s.clone()],
            EmbeddingInput::Batch(v) => v.clone(),
        };
        Self::Embedding { texts }
    }
}

// ─── From Conversions: mai-core -> API ──────────────────────────────

impl From<&mai_core::health::AdapterHealth> for AdapterHealthSummary {
    fn from(h: &mai_core::health::AdapterHealth) -> Self {
        let status = match &h.status {
            mai_core::health::AdapterStatus::Healthy => "healthy",
            mai_core::health::AdapterStatus::Degraded { .. } => "degraded",
            mai_core::health::AdapterStatus::Unhealthy { .. } => "unhealthy",
            mai_core::health::AdapterStatus::Unknown => "unknown",
        };
        Self {
            id: h.adapter_id.clone(),
            status: status.to_string(),
            avg_latency_ms: h.avg_latency_ms,
            error_rate: h.error_rate,
        }
    }
}

impl From<&mai_core::health::GpuHealth> for GpuHealthSummary {
    #[allow(clippy::cast_precision_loss)]
    fn from(g: &mai_core::health::GpuHealth) -> Self {
        let vram_pct = if g.vram_total > 0 {
            (g.vram_used as f32 / g.vram_total as f32) * 100.0
        } else {
            0.0
        };
        let thermal = match g.thermal_state {
            mai_core::health::ThermalState::Normal => "normal",
            mai_core::health::ThermalState::Elevated => "elevated",
            mai_core::health::ThermalState::Throttled => "throttled",
            mai_core::health::ThermalState::Critical => "critical",
        };
        Self {
            device_id: g.device_id.clone(),
            temperature_celsius: g.temperature_celsius,
            vram_utilization_percent: vram_pct,
            power_watts: g.power_watts,
            thermal_state: thermal.to_string(),
        }
    }
}

pub fn alert_level_to_string(level: mai_core::health::AlertLevel) -> String {
    level.as_str().to_string()
}

impl From<&mai_core::registry::CapabilityInfo> for ModelCapabilities {
    fn from(c: &mai_core::registry::CapabilityInfo) -> Self {
        Self {
            chat: c.chat,
            completion: c.completion,
            embedding: c.embedding,
            vision: c.vision,
            structured_output: c.structured_output,
            max_context_tokens: c.max_context_tokens,
        }
    }
}

pub fn power_state_to_string(state: mai_core::power::PowerState) -> String {
    state.as_str().to_string()
}
