"""TGI (Text Generation Inference) backend adapter.

HuggingFace's production inference server with quantization (bitsandbytes,
GPTQ, AWQ), speculative decoding, watermarking for compliance audit trails,
and Flash Attention optimization.

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
from adapters.tgi.client import TgiClient
from adapters.tgi.config import TgiConfig

logger = logging.getLogger("mai.adapters.tgi")


@mai_adapter(name="tgi", version="1.0.0")
class TgiAdapter(AdapterBase):
    """HuggingFace Text Generation Inference adapter.

    Supports quantization configs, speculative decoding with draft models,
    watermarking for compliance, and Flash Attention. TGI serves one model
    per instance; multi-model requires multiple TGI processes.
    """

    def __init__(self) -> None:
        super().__init__()
        self._client: TgiClient | None = None
        self._config: TgiConfig = TgiConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model_id: str = ""
        self._max_input_tokens: int = 4096
        self._max_total_tokens: int = 8192

    async def initialize(self, config: dict[str, Any], hil_handle: Any) -> str:
        """Initialize TGI adapter. Queries /info for model metadata."""
        self._config = TgiConfig.from_dict(config)
        self._hil_handle = hil_handle
        self._client = TgiClient(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )

        # Verify health
        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Get model info
        info = await asyncio.to_thread(self._client.info)
        self._model_id = info.get("model_id", self._config.default_model)
        self._max_input_tokens = info.get("max_input_length", self._config.max_input_tokens)
        self._max_total_tokens = info.get("max_total_tokens", self._config.max_total_tokens)

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            f"TGI adapter initialized: model={self._model_id}, "
            f"quantize={self._config.quantize}, speculate={self._config.speculate}"
        )
        return f"tgi-{self._model_id}-{self._start_time_ms}"

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError()

    async def generate(self, prompt: str, params: GenerationParams) -> AsyncIterator[Token]:
        """Stream tokens from TGI."""
        self._ensure_initialized()
        assert self._client is not None

        chunks = await asyncio.to_thread(
            self._client.generate,
            inputs=prompt,
            max_new_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            watermark=self._config.watermark,
            stream=True,
        )

        token_index = 0
        for chunk in chunks:
            if chunk.token_text:
                is_end = chunk.finish_reason is not None or chunk.generated_text is not None
                yield Token(
                    text=chunk.token_text,
                    index=token_index,
                    is_end_of_text=is_end,
                )
                token_index += 1

        self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation via sequential calls (TGI batches internally)."""
        self._ensure_initialized()
        assert self._client is not None

        results: list[GenerationResult] = []
        for prompt in prompts:
            resp = await asyncio.to_thread(
                self._client.generate,
                inputs=prompt,
                max_new_tokens=params.max_tokens,
                temperature=params.temperature,
                top_p=params.top_p,
                stop=params.stop_sequences or None,
                watermark=self._config.watermark,
                stream=False,
            )
            body = resp.body if isinstance(resp.body, dict) else {}
            generated = body.get("generated_text", "")
            details = body.get("details", {})
            tokens_out = details.get("generated_tokens", len(generated) // 4)
            finish = details.get("finish_reason", "stop_sequence")
            reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
            results.append(GenerationResult(
                text=generated, tokens_generated=tokens_out, finish_reason=reason,
            ))

        self._requests_served += len(prompts)
        return results

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """TGI does not natively support embeddings."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Health probe via /health endpoint."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await asyncio.to_thread(self._client.health)
        if healthy:
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """TGI capabilities: streaming, quantization, speculative decoding."""
        return AdapterCapabilities(
            max_context_window=self._max_total_tokens,
            supported_quantizations=["bitsandbytes", "gptq", "awq", "eetq", "fp8"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=True,
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version="2.0",
        )

    async def shutdown(self) -> None:
        """Graceful shutdown."""
        self._initialized = False
        self._client = None
        logger.info("TGI adapter shut down")
