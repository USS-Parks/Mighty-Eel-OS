"""MAI SDK type definitions.

All types are Pydantic v2 models that mirror the MAI API JSON schemas.
These types align with the internal Rust types defined in mai-core:

    API Type                -> Internal Rust Type
    ChatMessage             -> scheduler::ChatMessage
    Usage                   -> (computed at API boundary)
    FinishReason            -> adapter::FinishReason
    CapabilityInfo          -> registry::CapabilityInfo
    RequestPriority         -> scheduler::RequestPriority
    ErrorResponse.code      -> errors::CoreError variant

Session 05 deliverable. Full validation logic in Session 11.
"""

from __future__ import annotations

from datetime import datetime
from enum import StrEnum
from typing import Any
from uuid import UUID

from pydantic import BaseModel, Field


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------

class RequestPriority(StrEnum):
    """Maps to scheduler::RequestPriority."""
    LOW = "low"
    NORMAL = "normal"
    HIGH = "high"
    CRITICAL = "critical"


class FinishReason(StrEnum):
    """Maps to adapter::FinishReason."""
    STOP = "stop"
    MAX_TOKENS = "max_tokens"
    STOP_SEQUENCE = "stop_sequence"
    TOOL_CALLS = "tool_calls"


class ProfileRole(StrEnum):
    """Family profile roles."""
    ADMIN = "admin"
    ADULT = "adult"
    TEEN = "teen"
    CHILD = "child"
    GUEST = "guest"


class ContentSafetyLevel(StrEnum):
    """Content safety filter levels."""
    NONE = "none"
    MODERATE = "moderate"
    STRICT = "strict"


class AdapterStatus(StrEnum):
    """Maps to health::AdapterStatus."""
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNHEALTHY = "unhealthy"
    UNKNOWN = "unknown"


class ThermalState(StrEnum):
    """Maps to health::ThermalState."""
    NORMAL = "normal"
    ELEVATED = "elevated"
    THROTTLED = "throttled"
    CRITICAL = "critical"


class NetworkState(StrEnum):
    """Maps to health::NetworkState."""
    AIR_GAP_COMPLIANT = "air_gap_compliant"
    CONNECTED = "connected"
    NON_COMPLIANT = "non_compliant"


class PowerState(StrEnum):
    """Maps to power::PowerState."""
    OFF = "off"
    DEEP_VAULT_SLEEP = "deep_vault_sleep"
    SENTINEL = "sentinel"
    FULL_INFERENCE = "full_inference"
    THERMAL_THROTTLE = "thermal_throttle"


class ModelStatus(StrEnum):
    """Maps to registry::ModelStatus."""
    COLD_STORAGE = "cold_storage"
    LOADING = "loading"
    LOADED = "loaded"
    ACTIVE = "active"
    EVICTING = "evicting"
    EVICTED = "evicted"


class ModelFormat(StrEnum):
    """Maps to registry::ModelFormat."""
    GGUF = "GGUF"
    SAFE_TENSORS = "SafeTensors"
    EXL2 = "EXL2"
    GPTQ = "GPTQ"


class MaiErrorType(StrEnum):
    """Error type classification."""
    INVALID_REQUEST = "invalid_request"
    AUTHENTICATION_FAILED = "authentication_failed"
    MODEL_UNAVAILABLE = "model_unavailable"
    PERMISSION_DENIED = "permission_denied"
    VALIDATION_ERROR = "validation_error"
    RATE_LIMITED = "rate_limited"
    CONTEXT_EXCEEDED = "context_exceeded"
    INTERNAL_ERROR = "internal_error"
    REQUEST_FAILED = "request_failed"
    OVERLOADED = "overloaded"
    AIR_GAP_VIOLATION = "air_gap_violation"
    POWER_STATE_UNAVAILABLE = "power_state_unavailable"
    TIMEOUT = "timeout"


# ---------------------------------------------------------------------------
# Shared types
# ---------------------------------------------------------------------------

class Usage(BaseModel):
    """Token usage statistics."""
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0


class ChatMessage(BaseModel):
    """Single chat message. Maps to scheduler::ChatMessage."""
    role: str  # system, user, assistant, tool
    content: str
    tool_call_id: str | None = None


# ---------------------------------------------------------------------------
# Request types
# ---------------------------------------------------------------------------

class ResponseFormat(BaseModel):
    """Structured output format specification."""
    type: str  # json_object, json_schema
    schema_: dict[str, Any] | None = Field(None, alias="schema")


class ToolDefinition(BaseModel):
    """Tool/function definition for function calling."""
    type: str = "function"
    function: dict[str, Any]


class ChatCompletionRequest(BaseModel):
    """POST /v1/chat/completions request body."""
    model: str
    messages: list[ChatMessage]
    temperature: float = 0.7
    top_p: float = 0.9
    max_tokens: int = 2048
    stream: bool = True
    stop: list[str] = Field(default_factory=list)
    response_format: ResponseFormat | None = None
    tools: list[ToolDefinition] | None = None
    tool_choice: str | dict[str, Any] = "auto"


class CompletionRequest(BaseModel):
    """POST /v1/completions request body."""
    model: str
    prompt: str
    temperature: float = 0.7
    top_p: float = 0.9
    max_tokens: int = 2048
    stream: bool = True
    stop: list[str] = Field(default_factory=list)


class EmbeddingRequest(BaseModel):
    """POST /v1/embeddings request body."""
    model: str
    input: str | list[str]


class StructuredRequest(BaseModel):
    """POST /v1/generate/structured request body."""
    model: str
    prompt: str
    schema_: dict[str, Any] = Field(alias="schema")
    temperature: float = 0.0


class FunctionCallRequest(BaseModel):
    """POST /v1/generate/function_call request body."""
    model: str
    messages: list[ChatMessage]
    functions: list[dict[str, Any]]


# ---------------------------------------------------------------------------
# Response types
# ---------------------------------------------------------------------------

class ChatChoice(BaseModel):
    """Single choice in a chat completion response."""
    index: int
    message: ChatMessage
    finish_reason: FinishReason


class ChatCompletionResponse(BaseModel):
    """POST /v1/chat/completions non-streaming response."""
    id: str
    object: str = "chat.completion"
    created: int
    model: str
    choices: list[ChatChoice]
    usage: Usage


class ChatCompletionChunk(BaseModel):
    """SSE streaming chunk for chat completion."""
    id: str
    object: str = "chat.completion.chunk"
    created: int
    model: str
    sequence: int = 0
    choices: list[dict[str, Any]]
    usage: Usage | None = None


class CompletionResponse(BaseModel):
    """POST /v1/completions response."""
    id: str
    object: str = "text_completion"
    created: int
    model: str
    choices: list[dict[str, Any]]
    usage: Usage


class EmbeddingData(BaseModel):
    """Single embedding in an embedding response."""
    object: str = "embedding"
    index: int
    embedding: list[float]
    input_tokens: int  # IM extension, maps to adapter::Embedding.input_tokens


class EmbeddingResponse(BaseModel):
    """POST /v1/embeddings response."""
    object: str = "list"
    data: list[EmbeddingData]
    model: str
    usage: Usage


class StructuredResponse(BaseModel):
    """POST /v1/generate/structured response."""
    id: str
    object: str = "structured_output"
    model: str
    output: dict[str, Any]
    usage: Usage
    schema_valid: bool


class FunctionCallResult(BaseModel):
    """Function call result in response."""
    name: str
    arguments: str  # JSON-encoded


class FunctionCallResponse(BaseModel):
    """POST /v1/generate/function_call response."""
    id: str
    object: str = "function_call"
    model: str
    function_call: FunctionCallResult
    usage: Usage


# ---------------------------------------------------------------------------
# Model types
# ---------------------------------------------------------------------------

class CapabilityInfo(BaseModel):
    """Model capabilities. Maps to registry::CapabilityInfo."""
    chat: bool = False
    completion: bool = False
    embedding: bool = False
    vision: bool = False
    structured_output: bool = False
    max_context_tokens: int = 0
    supported_languages: list[str] = Field(default_factory=list)


class SecurityInfo(BaseModel):
    """Model security metadata."""
    signature_algorithm: str
    integrity_verified: bool


class ModelObject(BaseModel):
    """Model listing entry. Maps to registry::ModelSummary."""
    id: str
    object: str = "model"
    created: int
    owned_by: str = "island-mountain"
    name: str
    version: str
    format: ModelFormat
    quantization: str | None = None
    size_bytes: int
    required_vram_bytes: int
    status: ModelStatus
    capabilities: CapabilityInfo
    compatible_backends: list[str] = Field(default_factory=list)
    security: SecurityInfo | None = None


class AdapterAssignment(BaseModel):
    """Current adapter assignment for a loaded model."""
    adapter_id: str
    gpu_id: str


class ModelDetail(ModelObject):
    """Detailed model info (extends ModelObject)."""
    adapter_assignment: AdapterAssignment | None = None
    vram_allocated_bytes: int = 0
    request_count: int = 0
    last_used: datetime | None = None


# ---------------------------------------------------------------------------
# Health types
# ---------------------------------------------------------------------------

class AdapterHealthEntry(BaseModel):
    """Per-adapter health. Maps to health::AdapterHealth."""
    status: AdapterStatus
    last_heartbeat: datetime
    missed_heartbeats: int = 0
    avg_latency_ms: float = 0.0
    error_rate_5min: float = 0.0
    vram_usage_bytes: int = 0
    active_requests: int = 0


class GpuHealthEntry(BaseModel):
    """Per-GPU health. Maps to health::GpuHealth."""
    temperature_celsius: float
    fan_speed_percent: int
    vram_used_bytes: int
    vram_total_bytes: int
    power_limit_watts: int
    compute_utilization_percent: int


class HealthResponse(BaseModel):
    """GET /v1/health response. Maps to health::HealthSnapshot."""
    status: str  # healthy, degraded, unhealthy
    air_gap_verified: bool
    power_state: PowerState
    uptime_seconds: int
    adapters: dict[str, Any]
    hardware: dict[str, Any]
    system: dict[str, Any]


class HardwareHealthResponse(BaseModel):
    """GET /v1/health/hardware response."""
    gpus: dict[str, GpuHealthEntry] = Field(default_factory=dict)
    power_draw_watts: float = 0.0
    thermal_state: ThermalState = ThermalState.NORMAL
    network_state: NetworkState = NetworkState.AIR_GAP_COMPLIANT


# ---------------------------------------------------------------------------
# Power types
# ---------------------------------------------------------------------------

class AutoDemotion(BaseModel):
    """Auto-demotion status."""
    enabled: bool
    idle_minutes_remaining: int | None = None
    next_state: PowerState | None = None


class PowerStateResponse(BaseModel):
    """GET /v1/power/state response."""
    state: PowerState
    estimated_power_watts: float
    auto_demotion: AutoDemotion
    promotion_available: bool
    promotion_latency_target_ms: int


# ---------------------------------------------------------------------------
# Profile types
# ---------------------------------------------------------------------------

class ProfilePermissions(BaseModel):
    """Profile permission set."""
    model_access: list[str] = Field(default_factory=lambda: ["*"])
    max_context_tokens: int | None = None
    allowed_endpoints: list[str] = Field(default_factory=lambda: ["*"])
    can_manage_models: bool = False
    can_manage_power: bool = False
    can_view_audit: bool = False
    can_manage_profiles: bool = False


class ContentSafety(BaseModel):
    """Content safety filter settings."""
    enabled: bool = False
    filter_level: ContentSafetyLevel = ContentSafetyLevel.NONE


class RateLimits(BaseModel):
    """Per-profile rate limits."""
    requests_per_minute: int | None = None
    tokens_per_hour: int | None = None


class ProfileObject(BaseModel):
    """Family profile. Stored in local SQLite via vault."""
    profile_id: UUID
    name: str
    role: ProfileRole
    created_at: datetime
    permissions: ProfilePermissions
    priority: RequestPriority = RequestPriority.NORMAL
    rate_limits: RateLimits = Field(default_factory=RateLimits)
    content_safety: ContentSafety = Field(default_factory=ContentSafety)


# ---------------------------------------------------------------------------
# Audit types
# ---------------------------------------------------------------------------

class AuditEntry(BaseModel):
    """Single audit log entry."""
    timestamp: datetime
    request_id: UUID
    profile_id: UUID
    endpoint: str
    method: str
    model: str | None = None
    adapter: str | None = None
    tokens_in: int = 0
    tokens_out: int = 0
    latency_ms: int = 0
    status_code: int = 200
    priority: RequestPriority = RequestPriority.NORMAL
    hash: str = ""
    prev_hash: str = ""


class AuditLogResponse(BaseModel):
    """GET /v1/audit/log response."""
    total_entries: int
    offset: int
    limit: int
    entries: list[AuditEntry]


# ---------------------------------------------------------------------------
# Error types
# ---------------------------------------------------------------------------

class ErrorDetail(BaseModel):
    """Error detail in error response."""
    code: str  # MAI-XYYY
    message: str
    type: MaiErrorType
    retry_after_seconds: int | None = None
    request_id: UUID | None = None


class ErrorResponse(BaseModel):
    """Standard error response wrapper."""
    error: ErrorDetail


class MaiError(Exception):
    """SDK exception wrapping a MAI API error response."""

    def __init__(self, response: ErrorResponse) -> None:
        self.response = response
        self.code = response.error.code
        self.error_type = response.error.type
        self.retry_after = response.error.retry_after_seconds
        super().__init__(response.error.message)

    @property
    def is_retryable(self) -> bool:
        """Whether the client should retry this request."""
        return self.error_type in {
            MaiErrorType.RATE_LIMITED,
            MaiErrorType.REQUEST_FAILED,
            MaiErrorType.OVERLOADED,
            MaiErrorType.POWER_STATE_UNAVAILABLE,
            MaiErrorType.TIMEOUT,
        }
