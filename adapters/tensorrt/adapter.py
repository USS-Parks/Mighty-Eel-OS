"""TensorRT-LLM backend adapter.

NVIDIA's highest-performance inference via Triton Inference Server with
TensorRT-LLM backend. Targets H100/H200 SXM5 hardware with NVLink awareness,
INT8/FP8 calibration, and inflight batching.

Session 09 deliverable.
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
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    UnsupportedOperationError,
    mai_adapter,
)
from adapters.tensorrt.client import TensorRtClient
from adapters.tensorrt.config import TensorRtConfig

logger = logging.getLogger("mai.adapters.tensorrt")


@mai_adapter(name="tensorrt", version="1.0.0")
class TensorRtAdapter(AdapterBase):
    """TensorRT-LLM backend adapter.

    Provides the highest throughput path for NVIDIA H100/H200 hardware.
    Manages TensorRT engine compilation lifecycle, inflight batching,
    and multi-GPU inference with NVLink awareness.
    """

    def __init__(self) -> None:
        super().__init__()
        self._client: TensorRtClient | None = None
        self._config: TensorRtConfig = TensorRtConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model_name: str = ""
        self._engine_ready: bool = False

    async def initialize(self, config: dict[str, Any], hil_handle: Any) -> str:
        """Initialize TensorRT-LLM adapter. Verifies Triton health and model readiness."""
        self._config = TensorRtConfig.from_dict(config)
        self._hil_handle = hil_handle
        self._client = TensorRtClient(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )

        # Verify Triton is ready
        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Check model readiness
        self._model_name = self._config.default_model
        model_ready = await asyncio.to_thread(self._client.model_ready, self._model_name)
        self._engine_ready = model_ready

        if not model_ready:
            logger.warning(f"TensorRT model '{self._model_name}' not ready; adapter degraded")

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            f"TensorRT-LLM adapter initialized: model={self._model_name}, "
            f"tp={self._config.tensor_parallel_size}, precision={self._config.precision}, "
            f"engine_ready={self._engine_ready}"
        )
        return f"tensorrt-{self._model_name}-{self._start_time_ms}"

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError()

    async def generate(self, prompt: str, params: GenerationParams) -> AsyncIterator[Token]:
        """Stream tokens from TensorRT-LLM via Triton."""
        self._ensure_initialized()
        assert self._client is not None

        chunks = await asyncio.to_thread(
            self._client.generate,
            model=self._model_name,
            prompt=prompt,
            max_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            stream=True,
        )

        token_index = 0
        for chunk in chunks:
            if chunk.text:
                yield Token(
                    text=chunk.text,
                    index=token_index,
                    is_end_of_text=chunk.finished,
                )
                token_index += 1
            elif chunk.finished:
                yield Token(text="", index=token_index, is_end_of_text=True)

        self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. TensorRT-LLM handles batching via inflight batching."""
        self._ensure_initialized()
        assert self._client is not None

        results: list[GenerationResult] = []
        for prompt in prompts:
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
            body = resp.body
            text = body.get("text_output", body.get("choices", [{}])[0].get("text", ""))
            tokens_out = body.get("output_tokens", len(text) // 4)
            results.append(GenerationResult(
                text=text, tokens_generated=tokens_out, finish_reason=FinishReason.STOP,
            ))

        self._requests_served += len(prompts)
        return results

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """TensorRT-LLM does not support embeddings."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Health probe via Triton /v2/health/ready."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await asyncio.to_thread(self._client.health)
        if healthy:
            uptime = int(time.time() * 1000) - self._start_time_ms
            if self._engine_ready:
                return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
            return HealthStatus.degraded(reason="engine not ready", uptime_ms=uptime)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """TensorRT-LLM capabilities: highest throughput, inflight batching, no embeddings."""
        return AdapterCapabilities(
            max_context_window=self._config.max_input_len + self._config.max_output_len,
            supported_quantizations=["fp16", "fp8", "int8", "int4"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=True,
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version="0.12.0",
        )

    async def shutdown(self) -> None:
        """Graceful shutdown."""
        self._initialized = False
        self._client = None
        logger.info("TensorRT-LLM adapter shut down")

    # ─── TensorRT-specific methods ────────────────────────────────────────

    async def is_engine_ready(self) -> bool:
        """Check if TensorRT engine is compiled and loaded."""
        self._ensure_initialized()
        assert self._client is not None
        ready = await asyncio.to_thread(self._client.model_ready, self._model_name)
        self._engine_ready = ready
        return ready

    async def get_model_metadata(self) -> dict[str, Any]:
        """Get Triton model metadata (inputs, outputs, config)."""
        self._ensure_initialized()
        assert self._client is not None
        return await asyncio.to_thread(self._client.model_metadata, self._model_name)
