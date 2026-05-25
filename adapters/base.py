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

import asyncio
import inspect
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

    def __init__(self, timeout_ms: int | str = 0):
        detail = (
            str(timeout_ms) if isinstance(timeout_ms, str) else f"Timed out after {timeout_ms}ms"
        )
        super().__init__(code="Timeout", detail=detail, timeout_ms=timeout_ms)


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

    def __init__(self, detail: str | None = None):
        super().__init__(code="BackendCrashed", detail=detail or "Backend process crashed")


class BackendUnavailableError(AdapterError):
    """Port/socket not listening."""

    def __init__(self, detail: str | None = None):
        super().__init__(code="BackendUnavailable", detail=detail or "Backend service unavailable")


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


# ─── Async Utilities ─────────────────────────────────────────────────────────


async def maybe_await(fn_or_result: Any, *args: Any, **kwargs: Any) -> Any:
    """Safely call a function that may be sync or async.

    When tests use AsyncMock, calling via asyncio.to_thread() wraps the
    coroutine in another thread, producing a nested coroutine that never
    gets awaited. This helper detects the situation and does the right thing:
    - If fn_or_result is callable: call it, then await if the result is a coroutine
    - If fn_or_result is already a coroutine: await it
    """
    if callable(fn_or_result) and not asyncio.iscoroutine(fn_or_result):
        result = fn_or_result(*args, **kwargs)
    else:
        result = fn_or_result
    if asyncio.iscoroutine(result) or inspect.isawaitable(result):
        return await result
    return result


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
    extra: dict[str, Any] = field(default_factory=dict)

    @property
    def stop(self) -> list[str]:
        """Alias for stop_sequences (backward compat)."""
        return self.stop_sequences


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
    __hash__ = None

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Embedding):
            return self.vector == other.vector and self.input_tokens == other.input_tokens
        if isinstance(other, list):
            return self.vector == other
        return NotImplemented


class HealthStatusKind(Enum):
    """Health status discriminant."""

    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNAVAILABLE = "unavailable"


class _HealthyDescriptor:
    """Descriptor that acts as a classmethod factory when accessed on the class
    and as a boolean property when accessed on an instance."""

    def __get__(self, obj: Any, objtype: Any = None) -> Any:
        if obj is None:
            # Class-level access: HealthStatus.healthy(uptime_ms=..., ...)
            # Return a bound callable that takes (uptime_ms, requests_served)
            def _factory(uptime_ms: int, requests_served: int) -> HealthStatus:
                return HealthStatus(
                    kind=HealthStatusKind.HEALTHY,
                    uptime_ms=uptime_ms,
                    requests_served=requests_served,
                )
            return _factory
        # Instance-level access: status.healthy -> bool
        return obj.kind in (HealthStatusKind.HEALTHY, HealthStatusKind.DEGRADED)


@dataclass
class HealthStatus:
    """Adapter health status with associated data.
    Mirrors Rust HealthStatus enum variants.

    For HEALTHY: uptime_ms and requests_served are populated.
    For DEGRADED: reason and uptime_ms are populated.
    For UNAVAILABLE: only kind is set.

    Usage:
        # Factory classmethods
        HealthStatus.healthy(uptime_ms=1000, requests_served=5)
        HealthStatus.degraded(reason="engine not ready", uptime_ms=500)
        HealthStatus.unavailable()

        # Instance property
        status.healthy  # True if HEALTHY or DEGRADED
        status.message  # Human-readable reason string
    """

    kind: HealthStatusKind
    uptime_ms: int = 0
    requests_served: int = 0
    reason: str | None = None

    # Dual-mode descriptor: class-level factory + instance-level bool property
    healthy = _HealthyDescriptor()

    @property
    def message(self) -> str:
        """Human-readable status message."""
        if self.reason:
            return self.reason
        return self.kind.value

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

    max_context_window: int = 0
    supported_quantizations: list[str] = field(default_factory=list)
    supports_streaming: bool = True
    supports_batching: bool = False
    supports_structured_output: bool = False
    supports_vision: bool = False
    supports_tool_calling: bool = False
    supports_continuous_batching: bool = False
    supports_embedding: bool = False
    supports_hot_swap: bool = False
    backend_version: str = "unknown"
    extra: dict[str, Any] = field(default_factory=dict)

    def __init__(self, **kwargs: Any) -> None:
        """Flexible init that accepts both canonical and alias field names."""
        # Handle aliases: supports_embeddings -> supports_embedding,
        # max_context_length -> max_context_window
        if "supports_embeddings" in kwargs and "supports_embedding" not in kwargs:
            kwargs["supports_embedding"] = kwargs.pop("supports_embeddings")
        elif "supports_embeddings" in kwargs:
            kwargs.pop("supports_embeddings")
        if "max_context_length" in kwargs and "max_context_window" not in kwargs:
            kwargs["max_context_window"] = kwargs.pop("max_context_length")
        elif "max_context_length" in kwargs:
            kwargs.pop("max_context_length")

        # Set defaults then override with provided kwargs
        self.max_context_window = kwargs.pop("max_context_window", 0)
        self.supported_quantizations = kwargs.pop("supported_quantizations", [])
        self.supports_streaming = kwargs.pop("supports_streaming", True)
        self.supports_batching = kwargs.pop("supports_batching", False)
        self.supports_structured_output = kwargs.pop("supports_structured_output", False)
        self.supports_vision = kwargs.pop("supports_vision", False)
        self.supports_tool_calling = kwargs.pop("supports_tool_calling", False)
        self.supports_continuous_batching = kwargs.pop("supports_continuous_batching", False)
        self.supports_embedding = kwargs.pop("supports_embedding", False)
        self.supports_hot_swap = kwargs.pop("supports_hot_swap", False)
        self.backend_version = kwargs.pop("backend_version", "unknown")
        self.extra = kwargs.pop("extra", {})

    @property
    def supports_embeddings(self) -> bool:
        """Alias for supports_embedding (plural form)."""
        return self.supports_embedding


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

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        self._initialized: bool = False
        self._config: dict[str, Any] = config or {}
        self._hil_handle: Any | None = None
        # J-12: late-bound config for ``async with`` lifecycle. Separate from
        # ``_config`` so concrete adapters that store typed config objects on
        # ``_config`` (e.g. Ollama → OllamaConfig) do not collide.
        self._entry_config: dict[str, Any] | None = (
            dict(config) if config is not None else None
        )
        self._entry_hil_handle: Any | None = None

    def _validate_generate_request(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool,
    ) -> None:
        if not isinstance(prompt, str):
            raise ValidationError("prompt must be a string")
        if not prompt.strip():
            raise ValidationError("prompt must not be empty")
        if len(prompt) > 200_000:
            raise ValidationError("prompt too large")

        if not isinstance(stream, bool):
            raise ValidationError("stream must be a bool")

        if not isinstance(params, GenerationParams):
            raise ValidationError("params must be GenerationParams")
        if not isinstance(params.max_tokens, int):
            raise ValidationError("max_tokens must be an int")
        if params.max_tokens <= 0:
            raise ValidationError("max_tokens must be > 0")
        if params.max_tokens > 1_000_000:
            raise ValidationError("max_tokens too large")

        if not isinstance(params.temperature, (int, float)):
            raise ValidationError("temperature must be a number")
        if params.temperature < 0 or params.temperature > 2.0:
            raise ValidationError("temperature out of range")

        if not isinstance(params.top_p, (int, float)):
            raise ValidationError("top_p must be a number")
        if params.top_p <= 0 or params.top_p > 1.0:
            raise ValidationError("top_p out of range")

        if params.structured_schema is not None and not isinstance(params.structured_schema, dict):
            raise ValidationError("structured_schema must be a dict when present")

        if not isinstance(params.stop_sequences, list):
            raise ValidationError("stop_sequences must be a list")
        if len(params.stop_sequences) > 32:
            raise ValidationError("too many stop sequences")
        for item in params.stop_sequences:
            if not isinstance(item, str):
                raise ValidationError("stop sequences must be strings")
            if len(item) > 200:
                raise ValidationError("stop sequence too long")

    def _validate_embed_request(self, texts: list[str]) -> None:
        caps = self.capabilities()
        if not caps.supports_embedding:
            raise UnsupportedOperationError("embed")
        if not isinstance(texts, list):
            raise ValidationError("texts must be a list of strings")
        if not texts:
            return
        if len(texts) > 2048:
            raise ValidationError("too many texts")
        total_chars = 0
        for text in texts:
            if not isinstance(text, str):
                raise ValidationError("texts must be strings")
            if not text.strip():
                raise ValidationError("texts must not include empty strings")
            if len(text) > 50_000:
                raise ValidationError("text too large")
            total_chars += len(text)
        if total_chars > 200_000:
            raise ValidationError("texts payload too large")

    def set_config(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> None:
        """Bind config + hil_handle for use by ``async with``.

        Usage:

            adapter = OllamaAdapter()
            adapter.set_config({"host": "127.0.0.1"}, hil_handle=hil)
            async with adapter as a:
                ...

        Idempotent; replaces any prior binding.
        """
        self._entry_config = dict(config) if config is not None else {}
        self._entry_hil_handle = hil_handle

    async def __aenter__(self) -> AdapterBase:
        """Enter the async context: call ``initialize(_entry_config, …)``."""
        if getattr(self, "_entry_config", None) is None:
            raise ValidationError(
                "config not set; call set_config() or pass config to the "
                "constructor before `async with`",
            )
        await self.initialize(
            self._entry_config,
            hil_handle=getattr(self, "_entry_hil_handle", None),
        )
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: Any,
    ) -> bool:
        """Exit the async context: call ``shutdown()``. Never suppresses."""
        try:
            await self.shutdown()
        except Exception:
            logger.exception("shutdown() raised during __aexit__")
        return False

    @abstractmethod
    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str | None:
        """Initialize adapter with config and HIL handle.
        Returns opaque adapter handle string.
        Blocks until backend is ready to serve.
        Both args are optional for backward compat (tests call with no args)."""
        ...

    @abstractmethod
    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from a single prompt.
        When stream=False (default): returns GenerationResult (awaitable).
        When stream=True: returns AsyncIterator[Token] (async-for).
        Backpressure managed by FFI bridge channel capacity."""
        ...

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
