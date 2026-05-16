"""MAI Python SDK for IM-OS applications.

Provides typed client access to the MAI API (REST and gRPC) for
building L4-L5 applications on top of the Model Abstraction Interface.

Session 05 deliverable: type stubs and client skeleton.
Full implementation in Session 11.
"""

__version__ = "0.1.0"

from mai.types import (
    ChatMessage,
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatChoice,
    CompletionRequest,
    CompletionResponse,
    EmbeddingRequest,
    EmbeddingResponse,
    EmbeddingData,
    StructuredRequest,
    StructuredResponse,
    FunctionCallRequest,
    FunctionCallResponse,
    ModelObject,
    ModelDetail,
    CapabilityInfo,
    HealthResponse,
    AdapterHealthEntry,
    HardwareHealthResponse,
    GpuHealthEntry,
    PowerStateResponse,
    ProfileObject,
    ProfilePermissions,
    AuditEntry,
    AuditLogResponse,
    Usage,
    MaiError,
    ErrorResponse,
    RequestPriority,
    FinishReason,
    ContentSafetyLevel,
    ProfileRole,
)
from mai.client import MaiClient, AsyncMaiClient

__all__ = [
    # Client
    "MaiClient",
    "AsyncMaiClient",
    # Request types
    "ChatMessage",
    "ChatCompletionRequest",
    "CompletionRequest",
    "EmbeddingRequest",
    "StructuredRequest",
    "FunctionCallRequest",
    # Response types
    "ChatCompletionResponse",
    "ChatChoice",
    "CompletionResponse",
    "EmbeddingResponse",
    "EmbeddingData",
    "StructuredResponse",
    "FunctionCallResponse",
    "Usage",
    # Model types
    "ModelObject",
    "ModelDetail",
    "CapabilityInfo",
    # Health types
    "HealthResponse",
    "AdapterHealthEntry",
    "HardwareHealthResponse",
    "GpuHealthEntry",
    # Power types
    "PowerStateResponse",
    # Profile types
    "ProfileObject",
    "ProfilePermissions",
    # Audit types
    "AuditEntry",
    "AuditLogResponse",
    # Error types
    "MaiError",
    "ErrorResponse",
    # Enums
    "RequestPriority",
    "FinishReason",
    "ContentSafetyLevel",
    "ProfileRole",
]
