"""MAI adapter base class and stable compatibility exports.

All backend adapters inherit from AdapterBase and implement its abstract
methods. Adapters are untrusted capsules in the Tock trust model:
sandboxed, crash-isolated, and without direct hardware access.

Adapters self-register using the @mai_adapter decorator:

    @mai_adapter(name="ollama", version="1.0.0")
    class OllamaAdapter(AdapterBase):
        ...
"""

from __future__ import annotations

import asyncio
import inspect
import logging
from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from typing import Any

from adapters.errors import (
    AdapterError,
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    ContextExceededError,
    HardwareFaultError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.types import (
    AdapterCapabilities,
    AdapterMetrics,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    HealthStatusKind,
    Token,
)

logger = logging.getLogger("mai.adapters.base")

# The largest prompt (in characters) an adapter accepts. The IPC runner sizes
# its stdin frame limit off this so a prompt within the cap round-trips instead
# of overrunning the reader; see adapters/runner.py MAX_FRAME_BYTES.
MAX_PROMPT_CHARS = 200_000
# Largest per-text length and total payload for an embeddings request.
MAX_EMBED_TEXT_CHARS = 50_000
MAX_EMBED_TOTAL_CHARS = 200_000


def _validate_stop_sequence(item: Any) -> None:
    if not isinstance(item, str):
        raise ValidationError("stop sequences must be strings")
    if len(item) > 200:
        raise ValidationError("stop sequence too long")


def _validated_embed_text_chars(text: Any) -> int:
    if not isinstance(text, str):
        raise ValidationError("texts must be strings")
    if not text.strip():
        raise ValidationError("texts must not include empty strings")
    if len(text) > MAX_EMBED_TEXT_CHARS:
        raise ValidationError("text too large")
    return len(text)


def _missing_context_config_message() -> str:
    return (
        "config not set; call set_config() or pass config to the "
        "constructor before `async with`"
    )


async def maybe_await(fn_or_result: Any, *args: Any, **kwargs: Any) -> Any:
    """Safely call a function that may be sync or async."""
    if callable(fn_or_result) and not asyncio.iscoroutine(fn_or_result):
        result = fn_or_result(*args, **kwargs)
    else:
        result = fn_or_result
    if asyncio.iscoroutine(result) or inspect.isawaitable(result):
        return await result
    return result


class AdapterBase(ABC):
    """Abstract base class for MAI backend adapters."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        self._initialized: bool = False
        self._config: dict[str, Any] = config or {}
        self._hil_handle: Any | None = None
        self._entry_config: dict[str, Any] | None = (
            dict(config) if config is not None else None
        )
        self._entry_hil_handle: Any | None = None

    def _validate_prompt(self, prompt: str) -> None:
        if not isinstance(prompt, str):
            raise ValidationError("prompt must be a string")
        if not prompt.strip():
            raise ValidationError("prompt must not be empty")
        if len(prompt) > MAX_PROMPT_CHARS:
            raise ValidationError("prompt too large")

    def _validate_stop_sequences(self, stop_sequences: list[Any]) -> None:
        if not isinstance(stop_sequences, list):
            raise ValidationError("stop_sequences must be a list")
        if len(stop_sequences) > 32:
            raise ValidationError("too many stop sequences")
        for item in stop_sequences:
            _validate_stop_sequence(item)

    def _validate_generation_params(self, params: GenerationParams) -> None:
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
        self._validate_stop_sequences(params.stop_sequences)

    def _validate_generate_request(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool,
    ) -> None:
        self._validate_prompt(prompt)
        if not isinstance(stream, bool):
            raise ValidationError("stream must be a bool")
        self._validate_generation_params(params)

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
            total_chars += _validated_embed_text_chars(text)
        if total_chars > MAX_EMBED_TOTAL_CHARS:
            raise ValidationError("texts payload too large")

    def set_config(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> None:
        """Bind config and hil_handle for use by async context management."""
        self._entry_config = dict(config) if config is not None else {}
        self._entry_hil_handle = hil_handle

    async def __aenter__(self) -> AdapterBase:
        """Enter the async context and call initialize(_entry_config, ...)."""
        if getattr(self, "_entry_config", None) is None:
            raise ValidationError(_missing_context_config_message())
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
        """Exit the async context by calling shutdown(). Never suppresses."""
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
        """Initialize adapter with config and HIL handle."""
        ...

    @abstractmethod
    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from a single prompt."""
        ...

    @abstractmethod
    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation."""
        ...

    @abstractmethod
    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings."""
        ...

    @abstractmethod
    async def health_check(self) -> HealthStatus:
        """Lightweight health probe."""
        ...

    @abstractmethod
    def capabilities(self) -> AdapterCapabilities:
        """Return static logical capabilities."""
        ...

    @abstractmethod
    async def shutdown(self) -> None:
        """Graceful shutdown."""
        ...


_ADAPTER_REGISTRY: dict[str, type[AdapterBase]] = {}


def mai_adapter(*, name: str, version: str = "1.0.0"):
    """Decorator to register an adapter class with the MAI framework."""

    def decorator(cls: type[AdapterBase]) -> type[AdapterBase]:
        if not issubclass(cls, AdapterBase):
            raise TypeError(f"{cls.__name__} must inherit from AdapterBase")
        cls._mai_adapter_name = name
        cls._mai_adapter_version = version
        _ADAPTER_REGISTRY[name] = cls
        logger.info("Registered adapter: %s v%s", name, version)
        return cls

    return decorator


def get_adapter(name: str) -> type[AdapterBase] | None:
    """Retrieve a registered adapter class by name."""
    return _ADAPTER_REGISTRY.get(name)


def list_adapters() -> dict[str, type[AdapterBase]]:
    """Return all registered adapters."""
    return dict(_ADAPTER_REGISTRY)


__all__ = [
    "MAX_EMBED_TEXT_CHARS",
    "MAX_EMBED_TOTAL_CHARS",
    "MAX_PROMPT_CHARS",
    "AdapterBase",
    "AdapterCapabilities",
    "AdapterError",
    "AdapterMetrics",
    "AdapterTimeoutError",
    "BackendCrashedError",
    "BackendUnavailableError",
    "ContextExceededError",
    "Embedding",
    "FinishReason",
    "GenerationParams",
    "GenerationResult",
    "HardwareFaultError",
    "HealthStatus",
    "HealthStatusKind",
    "ModelNotFoundError",
    "OutOfMemoryError",
    "RateLimitedError",
    "Token",
    "UnsupportedOperationError",
    "ValidationError",
    "get_adapter",
    "list_adapters",
    "mai_adapter",
    "maybe_await",
]
