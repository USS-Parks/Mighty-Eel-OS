"""TensorRT-LLM backend adapter.

NVIDIA's highest-throughput local-inference path via Triton Inference
Server with the TensorRT-LLM backend. Targets H100/H200 SXM5 hardware
with NVLink awareness, inflight batching, and INT8/FP8 quantization.

Refactored under DOUGHERTY J-22 to satisfy
``docs/ADAPTER-SHARED-CONTRACT.md`` and
``docs/ADAPTER-TEST-HARNESS-LOCK.md``:

- ``__init__`` stores config only; no sockets, no network calls
- ``initialize`` validates config, creates a pooled client, probes
  Triton readiness, and is safe to call again after ``shutdown``
- ``generate(stream=False)`` returns ``GenerationResult``
- ``generate(stream=True)`` returns an ``AsyncIterator[Token]`` that
  yields lazily as Triton emits SSE frames -- no buffering
- ``generate_batch`` preserves input order, validates empty input,
  applies bounded concurrency
- ``embed`` raises ``UnsupportedOperationError`` (TensorRT-LLM has no
  embedding endpoint on the Triton TRT-LLM backend)
- ``health_check`` reports the 3 contract states (healthy / degraded /
  unavailable) and is cheap (no generation)
- ``capabilities`` truthfully reflects the implemented code paths
- ``shutdown`` closes the client and is idempotent; post-shutdown calls
  fail deterministically with a typed adapter error
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
    AdapterError,
    BackendCrashedError,
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
)
from adapters.tensorrt.client import TensorRtClient, TritonResponse, TritonStreamChunk
from adapters.tensorrt.config import TensorRtConfig

logger = logging.getLogger("mai.adapters.tensorrt")

_STREAM_SENTINEL: Any = object()
_BATCH_PARALLELISM_DEFAULT: int = 8


@mai_adapter(name="tensorrt", version="1.0.0")
class TensorRtAdapter(AdapterBase):
    """TensorRT-LLM backend adapter.

    Provides the highest-throughput path for NVIDIA H100/H200 hardware.
    Manages a single pooled HTTP client over Triton's KFServing API.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: TensorRtClient | None = None
        self._config: TensorRtConfig = (
            TensorRtConfig.from_dict(config) if config else TensorRtConfig()
        )
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model_name: str = ""
        self._engine_ready: bool = False

    # ─── Lifecycle ────────────────────────────────────────────────────────

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Initialize the adapter. Probes Triton readiness and model state."""
        if config is not None:
            self._config = TensorRtConfig.from_dict(config)
        _validate_config(self._config)

        if hil_handle is not None:
            self._hil_handle = hil_handle

        # Re-init after a prior shutdown is allowed; build a fresh client.
        self._client = TensorRtClient(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )

        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            # Tear down the client we just built so we don't leak state.
            self._client.close()
            self._client = None
            raise BackendUnavailableError(
                detail=f"Triton not ready at {self._config.base_url}",
            )

        self._model_name = self._config.default_model
        self._engine_ready = await asyncio.to_thread(
            self._client.model_ready, self._model_name,
        )
        if not self._engine_ready:
            logger.warning(
                "TensorRT model %r not ready on Triton; adapter starts degraded",
                self._model_name,
            )

        self._start_time_ms = _now_ms()
        self._requests_served = 0
        self._initialized = True
        logger.info(
            "TensorRT-LLM adapter initialized: model=%s, tp=%d, precision=%s, "
            "engine_ready=%s",
            self._model_name,
            self._config.tensor_parallel_size,
            self._config.precision,
            self._engine_ready,
        )
        return f"tensorrt-{self._model_name}-{self._start_time_ms}"

    async def shutdown(self) -> None:
        """Release the pooled client. Idempotent."""
        if self._client is not None:
            try:
                self._client.close()
            finally:
                self._client = None
        self._initialized = False
        self._engine_ready = False
        logger.info("TensorRT-LLM adapter shut down")

    # ─── Generation ───────────────────────────────────────────────────────

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from TensorRT-LLM.

        ``await adapter.generate(...)`` returns a ``GenerationResult``.
        ``async for tok in adapter.generate(..., stream=True)`` streams
        tokens as Triton emits them.
        """
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        if stream:
            return self._generate_stream(prompt, params)
        return await self._generate_one(prompt, params)

    async def _generate_one(
        self, prompt: str, params: GenerationParams,
    ) -> GenerationResult:
        """Single non-streaming generation."""
        assert self._client is not None
        resp = await asyncio.to_thread(
            self._client.generate,
            model=self._model_name,
            prompt=prompt,
            max_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            stream=False,
        )
        if not isinstance(resp, TritonResponse):
            raise BackendCrashedError(
                detail="non-streaming generate received a stream iterator",
            )
        text, tokens_out, finish = _result_from_body(resp.body, params)
        self._requests_served += 1
        return GenerationResult(
            text=text, tokens_generated=tokens_out, finish_reason=finish,
        )

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from Triton's SSE generate_stream endpoint."""
        assert self._client is not None
        # ``client.generate(..., stream=True)`` returns a sync iterator
        # built around an open HTTP response. Construction itself can
        # block (handshake + headers), so wrap it in to_thread. After
        # construction we step the iterator one chunk at a time, each
        # ``next`` going through to_thread to avoid blocking the loop.
        stream = await asyncio.to_thread(
            self._client.generate,
            model=self._model_name,
            prompt=prompt,
            max_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            stream=True,
        )
        if isinstance(stream, TritonResponse):
            raise BackendCrashedError(
                detail="stream generate received a non-streaming response",
            )

        token_index = 0
        try:
            while True:
                chunk = await asyncio.to_thread(_next_or_sentinel, stream)
                if chunk is _STREAM_SENTINEL:
                    break
                assert isinstance(chunk, TritonStreamChunk)
                if chunk.text:
                    yield Token(
                        text=chunk.text,
                        index=token_index,
                        is_end_of_text=chunk.finished,
                    )
                    token_index += 1
                if chunk.finished:
                    break
            # Always close with an end-of-text marker (consumer contract).
            yield Token(text="", index=token_index, is_end_of_text=True)
        finally:
            # If the underlying iterator is a generator with a .close(),
            # tell it to clean up its response.
            close = getattr(stream, "close", None)
            if callable(close):
                await asyncio.to_thread(close)
            self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. Triton TRT-LLM does inflight batching server-side;
        the adapter issues requests with bounded parallelism so the GPU stays
        fed without unbounded task creation.
        """
        self._ensure_initialized()
        if not prompts:
            return []

        parallelism = max(
            1,
            min(len(prompts), self._config.max_concurrent_requests or _BATCH_PARALLELISM_DEFAULT),
        )
        sem = asyncio.Semaphore(parallelism)

        async def _one(index: int, prompt: str) -> tuple[int, GenerationResult]:
            async with sem:
                result = await self._generate_one(prompt, params)
                return index, result

        tasks = [_one(i, p) for i, p in enumerate(prompts)]
        completed = await asyncio.gather(*tasks)
        completed.sort(key=lambda pair: pair[0])
        return [r for _, r in completed]

    # ─── Embedding (unsupported) ──────────────────────────────────────────

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """The Triton TensorRT-LLM backend does not expose an embedding endpoint."""
        # Even pre-init: the operation is unsupported in all states, so
        # we don't gate this on ``_ensure_initialized``. Tests can assert
        # the raise without paying for a client.
        raise UnsupportedOperationError("embedding")

    # ─── Health ───────────────────────────────────────────────────────────

    async def health_check(self) -> HealthStatus:
        """Cheap Triton readiness probe."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            return HealthStatus.unavailable()

        # Re-check model readiness lazily so transitions from "loading"
        # to "ready" surface without forcing a re-initialize.
        engine_now = await asyncio.to_thread(
            self._client.model_ready, self._model_name,
        )
        self._engine_ready = engine_now

        uptime = _now_ms() - self._start_time_ms
        if engine_now:
            return HealthStatus.healthy(
                uptime_ms=uptime, requests_served=self._requests_served,
            )
        return HealthStatus.degraded(
            reason=f"model {self._model_name!r} not ready on Triton",
            uptime_ms=uptime,
        )

    # ─── Capabilities ─────────────────────────────────────────────────────

    def capabilities(self) -> AdapterCapabilities:
        """Truthful capabilities for the implemented code paths."""
        return AdapterCapabilities(
            max_context_window=self._config.max_input_len + self._config.max_output_len,
            supported_quantizations=["fp16", "fp8", "int8", "int4"],
            supports_streaming=True,
            # Adapter does bounded parallel issuance over Triton's
            # inflight batcher -- both are real, both are tested.
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=bool(self._config.enable_inflight_batching),
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version="0.12.0",
            extra={
                "inflight_batching": self._config.enable_inflight_batching,
                "tensor_parallel_size": self._config.tensor_parallel_size,
                "precision": self._config.precision,
            },
        )

    # ─── TensorRT-specific helpers ────────────────────────────────────────

    async def is_engine_ready(self) -> bool:
        """Whether the TensorRT engine for the active model is loaded."""
        self._ensure_initialized()
        assert self._client is not None
        ready = await asyncio.to_thread(
            self._client.model_ready, self._model_name,
        )
        self._engine_ready = ready
        return ready

    async def get_model_metadata(self) -> dict[str, Any]:
        """Triton model metadata for the active model (inputs/outputs/config)."""
        self._ensure_initialized()
        assert self._client is not None
        return await asyncio.to_thread(
            self._client.model_metadata, self._model_name,
        )

    # ─── Internal helpers ─────────────────────────────────────────────────

    def _ensure_initialized(self) -> None:
        """Raise a typed adapter error if called before/after initialization."""
        if not self._initialized or self._client is None:
            raise AdapterError(
                code="NotReady",
                detail="Adapter not initialized. Call initialize() first.",
            )


# ─── Module-local helpers ──────────────────────────────────────────────────


def _validate_config(config: TensorRtConfig) -> None:
    """Reject obviously-invalid TensorRT configs with a typed ValidationError."""
    if not config.host:
        raise ValidationError("host must be set")
    if config.port <= 0 or config.port > 65535:
        raise ValidationError(f"port out of range: {config.port}")
    if config.timeout_ms <= 0:
        raise ValidationError(f"timeout_ms must be positive: {config.timeout_ms}")
    if config.stream_timeout_ms <= 0:
        raise ValidationError(
            f"stream_timeout_ms must be positive: {config.stream_timeout_ms}",
        )
    if not config.default_model:
        raise ValidationError("default_model must be set (Triton model name)")


def _result_from_body(
    body: dict[str, Any], params: GenerationParams,
) -> tuple[str, int, FinishReason]:
    """Extract ``(text, tokens_generated, finish_reason)`` from a Triton body."""
    text = ""
    if "text_output" in body:
        text = body.get("text_output") or ""
    elif "choices" in body:
        choices = body.get("choices") or []
        if choices and isinstance(choices[0], dict):
            text = choices[0].get("text") or choices[0].get("message", {}).get(
                "content", "",
            ) or ""
    tokens_out_raw = body.get("output_tokens") or body.get("generated_tokens")
    if isinstance(tokens_out_raw, int) and tokens_out_raw >= 0:
        tokens_out = tokens_out_raw
    else:
        tokens_out = max(1, len(text) // 4) if text else 0
    finish_raw = body.get("finish_reason") or body.get("stop_reason") or "stop"
    finish = _map_finish_reason(finish_raw, tokens_out, params)
    return text, tokens_out, finish


def _map_finish_reason(
    raw: str, tokens_out: int, params: GenerationParams,
) -> FinishReason:
    """Map Triton's finish-reason string into the MAI enum."""
    if raw in ("length", "max_tokens"):
        return FinishReason.MAX_TOKENS
    if raw == "stop_sequence" or (params.stop_sequences and raw == "stop"):
        # When stop sequences are configured AND backend reports "stop",
        # we can't always tell which fired; conservatively report STOP.
        return FinishReason.STOP_SEQUENCE if raw == "stop_sequence" else FinishReason.STOP
    if tokens_out >= params.max_tokens:
        return FinishReason.MAX_TOKENS
    return FinishReason.STOP


def _next_or_sentinel(iterator: Any) -> Any:
    """``next(iterator)`` that returns a sentinel on StopIteration.

    asyncio.to_thread can't propagate StopIteration cleanly, so we wrap
    the sync ``next`` call and surface end-of-stream as a sentinel.
    """
    try:
        return next(iterator)
    except StopIteration:
        return _STREAM_SENTINEL


def _now_ms() -> int:
    return int(time.time() * 1000)
