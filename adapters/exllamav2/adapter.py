"""ExLlamaV2 backend adapter.

EXL2 and GPTQ quantized inference with multi-model multiplexing on
single GPU. Optimized for memory-efficient quantized serving with
paged KV cache.

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
    maybe_await,
)
from adapters.exllamav2.client import ExLlamaV2Client
from adapters.exllamav2.config import ExLlamaV2Config

logger = logging.getLogger("mai.adapters.exllamav2")


@mai_adapter(name="exllamav2", version="1.0.0")
class ExLlamaV2Adapter(AdapterBase):
    """ExLlamaV2 backend adapter.

    Specializes in EXL2 and GPTQ quantized models with multi-model
    multiplexing on a single GPU. Uses paged KV cache for memory efficiency,
    enabling larger effective context on limited VRAM.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: ExLlamaV2Client | None = None
        self._config: ExLlamaV2Config = ExLlamaV2Config()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model: str = ""
        self._loaded_models: list[str] = []

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Initialize ExLlamaV2 adapter. Queries loaded models."""
        if config is not None:
            self._config = ExLlamaV2Config.from_dict(config)
        elif hasattr(self, "_cfg") and self._cfg is not None:
            self._config = self._cfg
        if hil_handle is not None:
            self._hil_handle = hil_handle
        if self._client is None:
            self._client = ExLlamaV2Client(
                base_url=self._config.base_url,
                timeout_ms=self._config.timeout_ms,
                stream_timeout_ms=self._config.stream_timeout_ms,
            )

        # Verify health
        healthy = await maybe_await(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Discover loaded models
        models_data = await maybe_await(self._client.models)
        if isinstance(models_data, dict):
            models_data = models_data.get("data", [])
        self._loaded_models = [m.get("id", "") for m in models_data]
        self._model = self._config.default_model or (
            self._loaded_models[0] if self._loaded_models else ""
        )

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            f"ExLlamaV2 adapter initialized: model={self._model}, "
            f"quant={self._config.quantization}, cache={self._config.cache_mode}, "
            f"loaded_models={len(self._loaded_models)}",
        )
        return f"exllamav2-{self._start_time_ms}"

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError()

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from ExLlamaV2. Dual-mode: await for result, async-for for streaming."""
        self._ensure_initialized()
        assert self._client is not None

        if stream:
            return self._generate_stream(prompt, params)

        # Non-streaming: return GenerationResult
        messages = [{"role": "user", "content": prompt}]
        resp = await maybe_await(
            self._client.chat_completions,
            model=self._model,
            messages=messages,
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=False,
        )
        if isinstance(resp, dict):
            body = resp
        else:
            body = resp.body if hasattr(resp, "body") else resp
        choices = body.get("choices", [])
        if choices:
            choice = choices[0]
            text = choice.get("message", {}).get("content", "")
            finish = choice.get("finish_reason", "stop")
            usage = body.get("usage", {})
            tokens_out = usage.get("completion_tokens", len(text) // 4)
            reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
        else:
            text, tokens_out, reason = "", 0, FinishReason.STOP

        self._requests_served += 1
        return GenerationResult(text=text, tokens_generated=tokens_out, finish_reason=reason)

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from ExLlamaV2."""
        assert self._client is not None
        messages = [{"role": "user", "content": prompt}]
        chunks = await asyncio.to_thread(
            self._client.chat_completions,
            model=self._model,
            messages=messages,
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=True,
        )

        token_index = 0
        for chunk in chunks:
            if chunk.content:
                yield Token(
                    text=chunk.content,
                    index=token_index,
                    is_end_of_text=chunk.finish_reason is not None,
                )
                token_index += 1
            elif chunk.finish_reason:
                yield Token(text="", index=token_index, is_end_of_text=True)

        self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation (sequential, ExLlamaV2 handles dynamic batching internally)."""
        self._ensure_initialized()
        assert self._client is not None

        results: list[GenerationResult] = []
        for prompt in prompts:
            messages = [{"role": "user", "content": prompt}]
            resp = await asyncio.to_thread(
                self._client.chat_completions,
                model=self._model,
                messages=messages,
                temperature=params.temperature,
                top_p=params.top_p,
                max_tokens=params.max_tokens,
                stop=params.stop_sequences or None,
                stream=False,
            )
            choices = resp.body.get("choices", [])
            if choices:
                choice = choices[0]
                text = choice.get("message", {}).get("content", "")
                finish = choice.get("finish_reason", "stop")
                usage = resp.body.get("usage", {})
                tokens_out = usage.get("completion_tokens", len(text) // 4)
                reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
                results.append(GenerationResult(
                    text=text, tokens_generated=tokens_out, finish_reason=reason,
                ))
            else:
                results.append(GenerationResult(text="", tokens_generated=0))

        self._requests_served += len(prompts)
        return results

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """ExLlamaV2 does not support embeddings."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Health probe."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await maybe_await(self._client.health)
        if healthy:
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """ExLlamaV2: quantized inference, multi-model, paged cache."""
        return AdapterCapabilities(
            max_context_window=self._config.max_seq_len,
            supported_quantizations=["exl2", "gptq"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=False,
            supports_embedding=False,
            supports_hot_swap=True,
            backend_version="0.2.0",
            extra={"multi_model": True},
        )

    async def shutdown(self) -> None:
        """Graceful shutdown."""
        self._initialized = False
        self._client = None
        logger.info("ExLlamaV2 adapter shut down")

    # ─── ExLlamaV2-specific methods ──────────────────────��───────────────

    async def load_model(self, model_name: str, config: dict[str, Any] | None = None) -> bool:
        """Load a model (supports multi-model multiplexing)."""
        _ = config
        self._ensure_initialized()
        assert self._client is not None
        try:
            await maybe_await(
                self._client.model_load, model_name, self._config.model_dir,
            )
            self._loaded_models.append(model_name)
            self._model = model_name
            return True
        except Exception as e:
            logger.warning(f"Failed to load model {model_name}: {e}")
            return False

    async def unload_model(self) -> bool:
        """Unload current model."""
        self._ensure_initialized()
        assert self._client is not None
        try:
            await maybe_await(self._client.model_unload)
            if self._model in self._loaded_models:
                self._loaded_models.remove(self._model)
            self._model = self._loaded_models[0] if self._loaded_models else ""
            return True
        except Exception as e:
            logger.warning(f"Failed to unload model: {e}")
            return False

    async def switch_model(self, model_name: str) -> bool:
        """Switch active model (must already be loaded)."""
        if model_name in self._loaded_models:
            self._model = model_name
            return True
        return False
