"""ExLlamaV2 backend adapter.

EXL2 and GPTQ quantized inference with multi-model multiplexing on
single GPU. Optimized for memory-efficient quantized serving with
paged KV cache.

"""

from __future__ import annotations

import logging
import time
from collections.abc import AsyncIterator
from typing import Any

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    BackendUnavailableError,
    Embedding,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    UnsupportedOperationError,
    mai_adapter,
    maybe_await,
)
from adapters.exllamav2.adapter_helpers import (
    batch_results,
    counted_stream,
    generation_result_from_body,
    stream_tokens,
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
        client = self._ensure_client()

        # Verify health
        healthy = await maybe_await(client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Discover loaded models
        models_data = await maybe_await(client.models)
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

    def _ensure_client(self) -> ExLlamaV2Client:
        """Return the live client, creating it once from the active config."""
        if self._client is not None:
            return self._client
        self._client = ExLlamaV2Client(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )
        return self._client

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
        self._validate_generate_request(prompt, params, stream=stream)
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
        self._requests_served += 1
        return generation_result_from_body(body)

    def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from ExLlamaV2."""
        assert self._client is not None
        stream = stream_tokens(self._client, self._model, prompt, params)
        return counted_stream(self, stream)

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation (sequential, ExLlamaV2 handles dynamic batching internally)."""
        self._ensure_initialized()
        assert self._client is not None

        results = await batch_results(self._client, self._model, prompts, params)
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
            await self._load_model_on_client(model_name)
            self._loaded_models.append(model_name)
            self._model = model_name
            return True
        except Exception as e:
            logger.warning(f"Failed to load model {model_name}: {e}")
            return False

    async def _load_model_on_client(self, model_name: str) -> None:
        assert self._client is not None
        await maybe_await(self._client.model_load, model_name, self._config.model_dir)

    async def unload_model(self) -> bool:
        """Unload current model."""
        self._ensure_initialized()
        assert self._client is not None
        try:
            await maybe_await(self._client.model_unload)
            self._remove_loaded_model(self._model)
            self._model = self._loaded_models[0] if self._loaded_models else ""
            return True
        except Exception as e:
            logger.warning(f"Failed to unload model: {e}")
            return False

    def _remove_loaded_model(self, model: str) -> None:
        if model in self._loaded_models:
            self._loaded_models.remove(model)

    async def switch_model(self, model_name: str) -> bool:
        """Switch active model (must already be loaded)."""
        if model_name in self._loaded_models:
            self._model = model_name
            return True
        return False
