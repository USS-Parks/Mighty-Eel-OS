"""vLLM backend adapter.

OpenAI-compatible inference via vLLM's native API server.
Primary adapter for Ranger and Pack Leader tiers (tensor parallelism,
continuous batching via PagedAttention, LoRA hot-loading).

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
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    mai_adapter,
    maybe_await,
)
from adapters.vllm.adapter_helpers import (
    body_from_response,
    chat_kwargs,
    chat_messages,
    embeddings_from_body,
    generation_result_from_body,
    resolve_default_model,
    token_from_chunk,
)
from adapters.vllm.client import VllmClient
from adapters.vllm.config import VllmConfig

logger = logging.getLogger("mai.adapters.vllm")


@mai_adapter(name="vllm", version="1.0.0")
class VllmAdapter(AdapterBase):
    """vLLM backend adapter.

    Leverages vLLM's OpenAI-compatible API with native tensor parallelism,
    continuous batching (PagedAttention), and LoRA hot-loading. Designed for
    multi-GPU Ranger/Pack Leader configurations.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: VllmClient | None = None
        self._config: VllmConfig = VllmConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model: str = ""
        self._available_models: list[str] = []

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Initialize vLLM adapter. Verifies server reachability and model availability."""
        if config is not None:
            self._config = VllmConfig.from_dict(config)
        elif hasattr(self, "_cfg") and self._cfg is not None:
            self._config = self._cfg
        if hil_handle is not None:
            self._hil_handle = hil_handle
        if self._client is None:
            self._client = VllmClient(
                base_url=self._config.base_url,
                timeout_ms=self._config.timeout_ms,
                stream_timeout_ms=self._config.stream_timeout_ms,
                health_check_timeout_ms=self._config.health_check_timeout_ms,
            )

        # Verify server is up
        healthy = await maybe_await(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Discover available models
        models_data = await maybe_await(self._client.models)
        if isinstance(models_data, dict):
            models_data = models_data.get("data", [])
        self._available_models = [m.get("id", "") for m in models_data]

        self._model = resolve_default_model(
            self._config.default_model, self._available_models,
        )

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            f"vLLM adapter initialized: model={self._model}, "
            f"tp={self._config.tensor_parallel_size}, "
            f"models_available={len(self._available_models)}",
        )
        return f"vllm-{self._model}-{self._start_time_ms}"

    def _ensure_initialized(self) -> None:
        """Guard against calls before initialization."""
        if not self._initialized or self._client is None:
            raise BackendUnavailableError()

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from vLLM. Dual-mode: await for result, async-for for streaming."""
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        assert self._client is not None

        if stream:
            return self._generate_stream(prompt, params)

        resp = await maybe_await(
            self._client.chat_completions,
            model=self._model,
            messages=chat_messages(prompt),
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=False,
            **chat_kwargs(params),
        )
        self._requests_served += 1
        return generation_result_from_body(body_from_response(resp))

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from vLLM via OpenAI-compatible SSE."""
        assert self._client is not None
        chunks = await asyncio.to_thread(
            self._client.chat_completions,
            model=self._model,
            messages=chat_messages(prompt),
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=True,
            **chat_kwargs(params),
        )

        token_index = 0
        for chunk in chunks:
            token = token_from_chunk(chunk, token_index)
            if token is None:
                continue
            yield token
            if token.text:
                token_index += 1

        self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. vLLM handles batching natively via continuous batching."""
        self._ensure_initialized()
        assert self._client is not None

        results: list[GenerationResult] = []
        for prompt in prompts:
            resp = await asyncio.to_thread(
                self._client.chat_completions,
                model=self._model,
                messages=chat_messages(prompt),
                temperature=params.temperature,
                top_p=params.top_p,
                max_tokens=params.max_tokens,
                stop=params.stop_sequences or None,
                stream=False,
            )
            results.append(generation_result_from_body(resp.body))

        self._requests_served += len(prompts)
        return results

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings via vLLM's /v1/embeddings endpoint."""
        self._ensure_initialized()
        self._validate_embed_request(texts)
        assert self._client is not None

        resp = await maybe_await(self._client.embeddings, texts)
        embeddings = embeddings_from_body(body_from_response(resp), texts)
        self._requests_served += 1
        return embeddings

    async def health_check(self) -> HealthStatus:
        """Lightweight health probe via /health endpoint."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await maybe_await(self._client.health)
        if healthy:
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """vLLM capabilities: continuous batching, streaming, structured output, embeddings."""
        cfg = getattr(self, "_cfg", None) or self._config
        return AdapterCapabilities(
            max_context_window=32768,
            supported_quantizations=["awq", "gptq", "squeezellm", "fp8"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=True,
            supports_vision=False,
            supports_tool_calling=True,
            supports_continuous_batching=True,
            supports_embedding=True,
            supports_hot_swap=True,
            backend_version="0.6.0",
            extra={"lora": cfg.enable_lora},
        )

    async def shutdown(self) -> None:
        """Graceful shutdown. vLLM server lifecycle is external."""
        self._initialized = False
        if self._client is not None:
            try:
                await maybe_await(self._client.close)
            except Exception:
                logger.warning("vLLM client close failed", exc_info=True)
        self._client = None
        logger.info("vLLM adapter shut down")

    # ─── vLLM-specific methods ────────────────────────────────────────────

    async def load_lora(self, lora_name: str, lora_path: str) -> bool:
        """Hot-load a LoRA adapter at runtime."""
        self._ensure_initialized()
        assert self._client is not None
        try:
            await maybe_await(self._client.lora_load, lora_name, lora_path)
            logger.info(f"LoRA loaded: {lora_name} from {lora_path}")
            return True
        except Exception as e:
            logger.warning(f"Failed to load LoRA {lora_name}: {e}")
            return False

    async def unload_lora(self, lora_name: str) -> bool:
        """Unload a LoRA adapter."""
        self._ensure_initialized()
        assert self._client is not None
        try:
            await maybe_await(self._client.lora_unload, lora_name)
            logger.info(f"LoRA unloaded: {lora_name}")
            return True
        except Exception as e:
            logger.warning(f"Failed to unload LoRA {lora_name}: {e}")
            return False

    async def list_models(self) -> list[str]:
        """List available models on vLLM server."""
        self._ensure_initialized()
        assert self._client is not None
        models_data = await maybe_await(self._client.models)
        return [m.get("id", "") for m in models_data]

    async def switch_model(self, model_id: str) -> bool:
        """Switch the active model for subsequent requests."""
        available = await self.list_models()
        if model_id in available:
            self._model = model_id
            return True
        return False
