"""MAI Adapter Base Class.

All backend adapters inherit from AdapterBase and implement its abstract
methods. Adapters are untrusted capsules in the Tock trust model: sandboxed,
crash-isolated, no direct hardware access.

Adapters self-register using the @mai_adapter decorator:

    @mai_adapter(name="ollama", version="1.0.0")
    class OllamaAdapter(AdapterBase):
        ...

Session 03 deliverable. Corrected per Claude audit (B1, B2, B3).
"""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from enum import Enum
from typing import Any

logger = logging.getLogger("mai.adapters.base")


# ─── Error Taxonomy (B2 fix: typed variants matching Rust AdapterError) ───────


class AdapterError(Exception):
    """Base exception for adapter failures. All variants carry structured data
    that maps 1:1 to Rust AdapterError enum variants across the FFI boundary."""

    def __init__(self, code: str, detail: str | None = None, **kwargs: Any):
        self.code = code
        self.detail = detail
        self.data = kwargs
        super().__init__(f"[{code}] {detail or ''}")


class AdapterTimeoutError(AdapterError):
    """Backend exceeded response deadline."""

    def __init__(self, timeout_ms: int):
        super().__init__(
            code="Timeout", detail=f"Timed out after {timeout_ms}ms", timeout_ms=timeout_ms,
        )


class OutOfMemoryError(AdapterError):
    """VRAM/Host memory exhausted."""

    def __init__(self):
        super().__init__(code="OutOfMemory", detail="Backend out of memory")


class ModelNotFoundError(AdapterError):
    """Requested model not loaded/available."""

    def __init__(self, model: str):
        super().__init__(code="ModelNotFound", detail=f"Model '{model}' not found", model=model)


class BackendCrashedError(AdapterError):
    """Adapter process terminated unexpectedly."""

    def __init__(self):
        super().__init__(code="BackendCrashed", detail="Backend process crashed")


class BackendUnavailableError(AdapterError):
    """Port/socket not listening."""

    def __init__(self):
        super().__init__(code="BackendUnavailable", detail="Backend service unavailable")


class ContextExceededError(AdapterError):
    """Prompt exceeds max_context_window."""

    def __init__(self, max_context: int):
        super().__init__(
            code="ContextExceeded", detail=f"Exceeds {max_context} tokens", max_context=max_context,
        )


class RateLimitedError(AdapterError):
    """Backend throttling active."""

    def __init__(self):
        super().__init__(code="RateLimited", detail="Backend rate limited")


class HardwareFaultError(AdapterError):
    """HIL-reported GPU/memristor failure."""

    def __init__(self, detail: str):
        super().__init__(code="HardwareFault", detail=detail)


class ValidationError(AdapterError):
    """Config/schema mismatch."""

    def __init__(self, reason: str):
        super().__init__(code="ValidationError", detail=reason, reason=reason)


class UnsupportedOperationError(AdapterError):
    """Operation not supported by this backend."""

    def __init__(self, operation: str):
        super().__init__(
            code="UnsupportedOperation", detail=f"Not supported: {operation}", operation=operation,
        )


# ─── Data Types (B1 fix: mirror Rust structs field-for-field) ─────────────────


@dataclass
class Token:
    """Single token output. Mirrors Rust Token struct exactly."""

    text: str
    logprob: float | None = None
    index: int = 0
    is_end_of_text: bool = False


@dataclass
class GenerationParams:
    """Generation parameters. Mirrors Rust GenerationParams."""

    temperature: float = 0.7
    top_p: float = 0.9
    max_tokens: int = 512
    stop_sequences: list[str] = field(default_factory=list)
    structured_schema: dict[str, Any] | None = None


class FinishReason(Enum):
    """Why generation stopped. Mirrors Rust FinishReason."""

    STOP = "stop"
    MAX_TOKENS = "max_tokens"
    STOP_SEQUENCE = "stop_sequence"


@dataclass
class GenerationResult:
    """Batch generation result. Mirrors Rust GenerationResult exactly."""

    text: str
    tokens_generated: int
    finish_reason: FinishReason = FinishReason.STOP


@dataclass
class Embedding:
    """Embedding vector output. Mirrors Rust Embedding exactly."""

    vector: list[float]
    input_tokens: int


class HealthStatusKind(Enum):
    """Health status discriminant."""

    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNAVAILABLE = "unavailable"


@dataclass
class HealthStatus:
    """Adapter health status with associated data.
    Mirrors Rust HealthStatus enum variants.

    For HEALTHY: uptime_ms and requests_served are populated.
    For DEGRADED: reason and uptime_ms are populated.
    For UNAVAILABLE: only kind is set.
    """

    kind: HealthStatusKind
    uptime_ms: int = 0
    requests_served: int = 0
    reason: str | None = None

    @classmethod
    def healthy(cls, uptime_ms: int, requests_served: int) -> HealthStatus:
        return cls(
            kind=HealthStatusKind.HEALTHY, uptime_ms=uptime_ms, requests_served=requests_served,
        )

    @classmethod
    def degraded(cls, reason: str, uptime_ms: int) -> HealthStatus:
        return cls(kind=HealthStatusKind.DEGRADED, reason=reason, uptime_ms=uptime_ms)

    @classmethod
    def unavailable(cls) -> HealthStatus:
        return cls(kind=HealthStatusKind.UNAVAILABLE)


# ─── Capabilities (B3 fix: hardware fields removed) ──────────────────────────


@dataclass
class AdapterCapabilities:
    """Static logical capabilities reported by adapter.

    NO hardware details. Hardware acceleration, VRAM budgets, and measured
    latency are the HIL's and AdapterManager's responsibility respectively.
    """

    max_context_window: int
    supported_quantizations: list[str]
    supports_streaming: bool = True
    supports_batching: bool = False
    supports_structured_output: bool = False
    supports_vision: bool = False
    supports_tool_calling: bool = False
    supports_continuous_batching: bool = False
    supports_embedding: bool = False
    supports_hot_swap: bool = False
    backend_version: str = "unknown"


# ─── Adapter Metrics ──────────────────────────────────────────────────────────


@dataclass
class AdapterMetrics:
    """Telemetry metrics reported per request or interval."""

    tokens_in: int = 0
    tokens_out: int = 0
    latency_ms: float = 0.0
    queue_depth: int = 0


# ─── Abstract Base Class ──────────────────────────────────────────────────────


class AdapterBase(ABC):
    """Abstract base class for MAI backend adapters.

    All methods mirror the Rust InferenceAdapter trait 1:1.
    Type signatures match the CBOR/MsgPack serialization contract.
    """

    def __init__(self) -> None:
        self._initialized: bool = False
        self._config: dict[str, Any] = {}
        self._hil_handle: Any | None = None

    @abstractmethod
    async def initialize(self, config: dict[str, Any], hil_handle: Any) -> str:
        """Initialize adapter with config and HIL handle.
        Returns opaque adapter handle string.
        Blocks until backend is ready to serve."""
        ...

    @abstractmethod
    async def generate(self, prompt: str, params: GenerationParams) -> AsyncIterator[Token]:
        """Stream tokens for a single prompt.
        Must be an async generator (use `yield`).
        Backpressure managed by FFI bridge channel capacity."""
        ...
        # Implementations use: async def generate(...) -> AsyncIterator[Token]:
        #     yield Token(text="...", is_end_of_text=True)

    @abstractmethod
    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. Returns typed GenerationResult list.
        Backends without native batching parallelize internally."""
        ...

    @abstractmethod
    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings. Returns typed Embedding list.
        Backends without embedding support MUST raise UnsupportedOperationError."""
        ...

    @abstractmethod
    async def health_check(self) -> HealthStatus:
        """Lightweight health probe (<100ms). Returns structured HealthStatus."""
        ...

    @abstractmethod
    def capabilities(self) -> AdapterCapabilities:
        """Return static logical capabilities. No hardware details."""
        ...

    @abstractmethod
    async def shutdown(self) -> None:
        """Graceful shutdown. Release resources via HIL handle."""
        ...


# ─── Registration ─────────────────────────────────────────────────────────────

_ADAPTER_REGISTRY: dict[str, type[AdapterBase]] = {}


def mai_adapter(*, name: str, version: str = "1.0.0"):
    """Decorator to register an adapter class with the MAI framework.

    Discovery: AdapterManager scans adapters/ for files containing this
    decorator via static AST parsing (no execution at discovery time).
    """

    def decorator(cls: type[AdapterBase]) -> type[AdapterBase]:
        if not issubclass(cls, AdapterBase):
            raise TypeError(f"{cls.__name__} must inherit from AdapterBase")
        cls._mai_adapter_name = name
        cls._mai_adapter_version = version
        _ADAPTER_REGISTRY[name] = cls
        logger.info(f"Registered adapter: {name} v{version}")
        return cls

    return decorator


def get_adapter(name: str) -> type[AdapterBase] | None:
    """Retrieve a registered adapter class by name."""
    return _ADAPTER_REGISTRY.get(name)


def list_adapters() -> dict[str, type[AdapterBase]]:
    """Return all registered adapters."""
    return dict(_ADAPTER_REGISTRY)
