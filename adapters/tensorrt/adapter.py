"""TensorRT-LLM backend adapter."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator
from typing import Any

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    AdapterError,
    BackendCrashedError,
    BackendUnavailableError,
    Embedding,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    UnsupportedOperationError,
    mai_adapter,
)
from adapters.tensorrt.adapter_helpers import (
    batch_parallelism,
    now_ms,
    result_from_body,
    stream_tokens_from_triton,
    validate_config,
)
from adapters.tensorrt.client import TensorRtClient, TritonResponse
from adapters.tensorrt.config import TensorRtConfig

logger = logging.getLogger("mai.adapters.tensorrt")

_BATCH_PARALLELISM_DEFAULT: int = 8


@mai_adapter(name="tensorrt", version="1.0.0")
class TensorRtAdapter(AdapterBase):
    """TensorRT-LLM adapter over Triton's KFServing API."""

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
        validate_config(self._config)

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

        self._start_time_ms = now_ms()
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
        text, tokens_out, finish = result_from_body(resp.body, params)
        self._requests_served += 1
        return GenerationResult(
            text=text, tokens_generated=tokens_out, finish_reason=finish,
        )

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from Triton's SSE generate_stream endpoint."""
        assert self._client is not None
        try:
            async for token in stream_tokens_from_triton(
                self._client, self._model_name, prompt, params,
            ):
                yield token
        finally:
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

        parallelism = batch_parallelism(
            len(prompts),
            self._config.max_concurrent_requests,
            _BATCH_PARALLELISM_DEFAULT,
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

        uptime = now_ms() - self._start_time_ms
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
