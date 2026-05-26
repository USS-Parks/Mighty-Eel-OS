"""MLX backend adapter.

Apple Silicon local inference via the `mlx-lm` package. MLX is
in-process — there is no HTTP/gRPC client and no remote backend; the
"backend" is a model directory on the operator's disk plus the loaded
Python handles inside this process.

Honest capabilities:
  - streaming: yes (mlx-lm.stream_generate)
  - non-streaming: yes (mlx-lm.generate)
  - batching: bounded adapter-level fan-out (no native batch)
  - embeddings: no (mlx-lm exposes no stable embedding endpoint)
  - structured output: no
  - tool calling: no
  - vision: no

Session J-25 (DOUGHERTY lane) deliverable. Conforms to
docs/ADAPTER-SHARED-CONTRACT.md and docs/ADAPTER-TEST-HARNESS-LOCK.md.
"""

from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import AsyncIterator
from typing import Any

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    AdapterTimeoutError,
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    ModelNotFoundError,
    Token,
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
    maybe_await,
)
from adapters.mlx.client import MLXClient, MLXLoadError, is_apple_silicon
from adapters.mlx.config import MLXConfig

logger = logging.getLogger("mai.adapters.mlx")


def _raise_load_error(error: MLXLoadError, model_path: str) -> None:
    msg = str(error).lower()
    if "not found" in msg or "no such" in msg:
        raise ModelNotFoundError(model_path) from error
    raise BackendUnavailableError(detail=str(error)) from error


async def _stream_tokens(
    chunks_iter: Any,
    deadline: float,
    timeout_ms: int,
) -> AsyncIterator[Token]:
    index = 0
    for chunk in chunks_iter:
        if time.monotonic() > deadline:
            raise AdapterTimeoutError(timeout_ms)
        if not chunk:
            continue
        yield Token(text=chunk, index=index, is_end_of_text=False)
        index += 1
    yield Token(text="", index=index, is_end_of_text=True)


async def _stream_from_client(
    client: MLXClient,
    prompt: str,
    params: GenerationParams,
    stream_timeout_ms: int,
) -> AsyncIterator[Token]:
    deadline = time.monotonic() + (stream_timeout_ms / 1000.0)
    chunks_iter = await asyncio.to_thread(
        client.stream_generate,
        prompt,
        max_tokens=params.max_tokens,
        temperature=params.temperature,
        top_p=params.top_p,
    )
    async for token in _stream_tokens(chunks_iter, deadline, stream_timeout_ms):
        yield token


async def _counted_stream(owner: Any, stream: AsyncIterator[Token]) -> AsyncIterator[Token]:
    try:
        async for token in stream:
            yield token
    except MLXLoadError as e:
        raise BackendUnavailableError(detail=str(e)) from e
    owner._requests_served += 1


def _lost_handle_status(start_time_ms: int) -> HealthStatus:
    return HealthStatus.degraded(
        reason="MLX client lost model handle",
        uptime_ms=int(time.time() * 1000) - start_time_ms,
    )


def _capability_extra() -> dict[str, Any]:
    return {
        "in_process": True,
        "apple_silicon_only": True,
        "platform_ok": is_apple_silicon(),
    }


async def _run_generate(
    client: MLXClient,
    prompt: str,
    params: GenerationParams,
    timeout_ms: int,
) -> tuple[str, int, bool]:
    return await asyncio.wait_for(
        asyncio.to_thread(
            client.generate,
            prompt,
            max_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
        ),
        timeout=timeout_ms / 1000.0,
    )


@mai_adapter(name="mlx", version="1.0.0")
class MLXAdapter(AdapterBase):
    """MLX backend adapter (Apple Silicon, in-process)."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: MLXClient | None = None
        # Build the typed config eagerly when a dict is supplied to
        # __init__ so callers do not have to pass it twice through
        # initialize(). The dict path on initialize() still wins when
        # both are supplied.
        self._config: MLXConfig = (
            MLXConfig.from_dict(config) if config else MLXConfig()
        )
        self._start_time_ms: int = 0
        self._requests_served: int = 0

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Validate config, instantiate the client, load the model.

        Raises:
          ValidationError: model_path missing or empty
          BackendUnavailableError: not on Apple Silicon or mlx-lm absent
          ModelNotFoundError: model_path does not resolve on disk
        """
        if config is not None:
            self._config = MLXConfig.from_dict(config)
        elif hasattr(self, "_cfg") and self._cfg is not None:
            self._config = self._cfg
        if hil_handle is not None:
            self._hil_handle = hil_handle

        if not self._config.model_path:
            raise ValidationError("MLX requires model_path")

        client = self._ensure_client()

        try:
            await asyncio.to_thread(client.load)
        except MLXLoadError as e:
            _raise_load_error(e, self._config.model_path)

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            "MLX adapter initialized: path=%s backend=%s",
            self._config.model_path,
            client.backend_version,
        )
        return f"mlx-{self._start_time_ms}"

    def _ensure_client(self) -> MLXClient:
        """Return the live client, creating it once from the active config."""
        if self._client is not None:
            return self._client
        self._client = MLXClient(
            model_path=self._config.model_path,
            tokenizer_path=self._config.tokenizer_path,
        )
        return self._client

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None or not self._client.loaded:
            raise BackendUnavailableError(detail="MLX adapter not initialized")

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Run a generation. Stream mode returns an async iterator."""
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        assert self._client is not None

        if stream:
            return self._generate_stream(prompt, params)

        try:
            text, tokens, hit_max = await self._generate_once(prompt, params)
        except TimeoutError as e:
            raise AdapterTimeoutError(self._config.timeout_ms) from e
        except MLXLoadError as e:
            raise BackendUnavailableError(detail=str(e)) from e

        self._requests_served += 1
        reason = FinishReason.MAX_TOKENS if hit_max else FinishReason.STOP
        return GenerationResult(
            text=text,
            tokens_generated=tokens,
            finish_reason=reason,
        )

    async def _generate_once(
        self,
        prompt: str,
        params: GenerationParams,
    ) -> tuple[str, int, bool]:
        assert self._client is not None
        return await _run_generate(self._client, prompt, params, self._config.timeout_ms)

    def _generate_stream(
        self,
        prompt: str,
        params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Yield tokens in order with a stream-level wall-clock budget."""
        assert self._client is not None
        stream = _stream_from_client(
            self._client, prompt, params, self._config.stream_timeout_ms,
        )
        return _counted_stream(self, stream)

    async def generate_batch(
        self,
        prompts: list[str],
        params: GenerationParams,
    ) -> list[GenerationResult]:
        """Bounded sequential batch. MLX-lm has no native batch surface."""
        self._ensure_initialized()
        assert self._client is not None

        if not prompts:
            return []

        results: list[GenerationResult] = []
        for prompt in prompts:
            r = await self.generate(prompt, params, stream=False)
            assert isinstance(r, GenerationResult)
            results.append(r)
        return results

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """MLX adapter does not implement embeddings."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Cheap probe — checks that the client still holds a model."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        if not self._client.loaded:
            return _lost_handle_status(self._start_time_ms)
        uptime = int(time.time() * 1000) - self._start_time_ms
        return HealthStatus.healthy(
            uptime_ms=uptime,
            requests_served=self._requests_served,
        )

    def capabilities(self) -> AdapterCapabilities:
        """Truthful capability flags for this MLX adapter."""
        backend_version = (
            self._client.backend_version if self._client is not None else "unknown"
        )
        return AdapterCapabilities(
            max_context_window=self._config.max_context_window,
            supported_quantizations=["4bit", "8bit", "fp16"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=False,
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version=backend_version,
            extra=_capability_extra(),
        )

    async def shutdown(self) -> None:
        """Idempotent shutdown — releases the model handle."""
        if self._client is not None:
            await maybe_await(self._client.close)
        self._client = None
        self._initialized = False
        logger.info("MLX adapter shut down")
