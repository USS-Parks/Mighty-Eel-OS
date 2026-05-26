"""Shared adapter contract data shapes."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


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
        """Alias for stop_sequences for backward compatibility."""
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
    """Factory on the class and bool property on instances."""

    def __get__(self, obj: Any, objtype: Any = None) -> Any:
        if obj is None:
            def _factory(uptime_ms: int, requests_served: int) -> HealthStatus:
                return HealthStatus(
                    kind=HealthStatusKind.HEALTHY,
                    uptime_ms=uptime_ms,
                    requests_served=requests_served,
                )
            return _factory
        return obj.kind in (HealthStatusKind.HEALTHY, HealthStatusKind.DEGRADED)


@dataclass
class HealthStatus:
    """Adapter health status with associated data."""

    kind: HealthStatusKind
    uptime_ms: int = 0
    requests_served: int = 0
    reason: str | None = None

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


@dataclass
class AdapterCapabilities:
    """Static logical capabilities reported by adapter."""

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
        """Flexible init that accepts canonical and alias field names."""
        if "supports_embeddings" in kwargs and "supports_embedding" not in kwargs:
            kwargs["supports_embedding"] = kwargs.pop("supports_embeddings")
        elif "supports_embeddings" in kwargs:
            kwargs.pop("supports_embeddings")
        if "max_context_length" in kwargs and "max_context_window" not in kwargs:
            kwargs["max_context_window"] = kwargs.pop("max_context_length")
        elif "max_context_length" in kwargs:
            kwargs.pop("max_context_length")

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
        """Alias for supports_embedding."""
        return self.supports_embedding


@dataclass
class AdapterMetrics:
    """Telemetry metrics reported per request or interval."""

    tokens_in: int = 0
    tokens_out: int = 0
    latency_ms: float = 0.0
    queue_depth: int = 0
