"""vLLM backend adapter.

OpenAI-compatible inference via vLLM's native API server.
Primary adapter for Ranger and Pack Leader tiers (tensor parallelism,
continuous batching via PagedAttention, LoRA hot-loading).

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
    mai_adapter,
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

    async def initialize(self, config: dict[str, Any], hil_handle: Any) -> str:
        """Initialize vLLM adapter. Verifies server reachability and model availability."""
        self._config = VllmConfig.from_dict(config)
        self._hil_handle = hil_handle
        self._client = VllmClient(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )

        # Verify server is up
        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Discover available models
        models_data = await asyncio.to_thread(self._client.models)
        self._available_models = [m.get("id", "") for m in models_data]

        # Set default model
        self._model = self._config.default_model
        if self._available_models and self._model not in self._available_models:
            # Try partial match
            for m in self._available_models:
                if self._model in m or m in self._model:
                    self._model = m
                    break
            else:
                if self._available_models:
                    self._model = self._available_models[0]

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

    async def generate(self, prompt: str, params: GenerationParams) -> AsyncIterator[Token]:
        """Stream tokens from vLLM via OpenAI-compatible SSE."""
        self._ensure_initialized()
        assert self._client is not None

        messages = [{"role": "user", "content": prompt}]

        # Build request kwargs
        kwargs: dict[str, Any] = {}
        if params.structured_schema:
            kwargs["guided_json"] = params.structured_schema

        chunks = await asyncio.to_thread(
            self._client.chat_completions,
            model=self._model,
            messages=messages,
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=True,
            **kwargs,
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
        """Batch generation. vLLM handles batching natively via continuous batching."""
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
            # resp is VllmResponse
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

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings via vLLM's /v1/embeddings endpoint."""
        self._ensure_initialized()
        assert self._client is not None

        body = {"model": self._model, "input": texts}
        resp = await asyncio.to_thread(
            self._client._request, "POST", "/v1/embeddings", body,
        )

        embeddings: list[Embedding] = []
        data = resp.body.get("data", [])
        usage = resp.body.get("usage", {})
        total_tokens = usage.get("total_tokens", sum(len(t) // 4 for t in texts))
        per_text_tokens = total_tokens // max(len(texts), 1)

        for item in data:
            vector = item.get("embedding", [])
            embeddings.append(Embedding(vector=vector, input_tokens=per_text_tokens))

        self._requests_served += 1
        return embeddings

    async def health_check(self) -> HealthStatus:
        """Lightweight health probe via /health endpoint."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await asyncio.to_thread(self._client.health)
        if healthy:
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """vLLM capabilities: continuous batching, streaming, structured output, embeddings."""
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
        )

    async def shutdown(self) -> None:
        """Graceful shutdown. vLLM server lifecycle is external."""
        self._initialized = False
        self._client = None
        logger.info("vLLM adapter shut down")

    # ─── vLLM-specific methods ────────────────────────────────────────────

    async def load_lora(self, lora_name: str, lora_path: str) -> bool:
        """Hot-load a LoRA adapter at runtime."""
        self._ensure_initialized()
        assert self._client is not None
        try:
            await asyncio.to_thread(self._client.lora_load, lora_name, lora_path)
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
            await asyncio.to_thread(self._client.lora_unload, lora_name)
            logger.info(f"LoRA unloaded: {lora_name}")
            return True
        except Exception as e:
            logger.warning(f"Failed to unload LoRA {lora_name}: {e}")
            return False

    async def list_models(self) -> list[str]:
        """List available models on vLLM server."""
        self._ensure_initialized()
        assert self._client is not None
        models_data = await asyncio.to_thread(self._client.models)
        return [m.get("id", "") for m in models_data]

    async def switch_model(self, model_id: str) -> bool:
        """Switch the active model for subsequent requests."""
        available = await self.list_models()
        if model_id in available:
            self._model = model_id
            return True
        return False
