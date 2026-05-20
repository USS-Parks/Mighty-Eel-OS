"""llama.cpp backend adapter.

Lightweight inference via llama-server. Critical for fallback deployments,
GGUF models, and grammar-constrained decoding. Supports Metal (Apple Silicon)
for development/test environments.

Session 09 deliverable.
"""

from __future__ import annotations

import asyncio
import json
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
from adapters.llamacpp.client import LlamaCppClient
from adapters.llamacpp.config import LlamaCppConfig

logger = logging.getLogger("mai.adapters.llamacpp")


@mai_adapter(name="llamacpp", version="1.0.0")
class LlamaCppAdapter(AdapterBase):
    """llama.cpp backend adapter.

    Wraps llama-server HTTP API. Specializes in GGUF model loading,
    grammar-constrained decoding (GBNF), and efficient CPU/GPU hybrid
    inference with configurable layer offloading.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: LlamaCppClient | None = None
        self._config: LlamaCppConfig = LlamaCppConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._context_size: int = 8192
        self._model_name: str = ""

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Initialize llama.cpp adapter. Verifies server health and gets model info."""
        if config is not None:
            self._config = LlamaCppConfig.from_dict(config)
        elif hasattr(self, "_cfg") and self._cfg is not None:
            self._config = self._cfg
        if hil_handle is not None:
            self._hil_handle = hil_handle
        if self._client is None:
            self._client = LlamaCppClient(
                base_url=self._config.base_url,
                timeout_ms=self._config.timeout_ms,
                stream_timeout_ms=self._config.stream_timeout_ms,
            )

        # Verify server health
        health = await maybe_await(self._client.health)
        if health.get("status") == "error":
            raise BackendUnavailableError()

        # Get server properties for context size
        props = await maybe_await(self._client.props)
        self._context_size = props.get("default_generation_settings", {}).get(
            "n_ctx", self._config.context_size
        )
        self._model_name = props.get("model_path", self._config.default_model)

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            f"llama.cpp adapter initialized: model={self._model_name}, "
            f"ctx={self._context_size}, gpu_layers={self._config.n_gpu_layers}",
        )
        return f"llamacpp-{self._start_time_ms}"

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
        """Generate from llama.cpp. Dual-mode: await for result, async-for for streaming."""
        self._ensure_initialized()
        assert self._client is not None

        if stream:
            return self._generate_stream(prompt, params)

        # Non-streaming: return GenerationResult
        messages = [{"role": "user", "content": prompt}]
        resp = await maybe_await(
            self._client.chat_completions,
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
        """Stream tokens from llama-server."""
        assert self._client is not None
        messages = [{"role": "user", "content": prompt}]

        # Use grammar if structured schema requested
        grammar = self._config.default_grammar
        if params.structured_schema:
            grammar = json.dumps(params.structured_schema) if not grammar else grammar

        chunks = await asyncio.to_thread(
            self._client.chat_completions,
            messages=messages,
            temperature=params.temperature,
            top_p=params.top_p,
            max_tokens=params.max_tokens,
            stop=params.stop_sequences or None,
            stream=True,
            grammar=grammar,
        )

        token_index = 0
        for chunk in chunks:
            if chunk.content:
                yield Token(
                    text=chunk.content,
                    index=token_index,
                    is_end_of_text=chunk.stop,
                )
                token_index += 1
            elif chunk.stop:
                yield Token(text="", index=token_index, is_end_of_text=True)

        self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. llama.cpp processes sequentially (no native batching)."""
        self._ensure_initialized()
        assert self._client is not None

        results: list[GenerationResult] = []
        for prompt in prompts:
            messages = [{"role": "user", "content": prompt}]
            resp = await asyncio.to_thread(
                self._client.chat_completions,
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
                reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
                tokens_out = resp.body.get("usage", {}).get("completion_tokens", len(text) // 4)
                results.append(GenerationResult(
                    text=text, tokens_generated=tokens_out, finish_reason=reason,
                ))
            else:
                results.append(GenerationResult(text="", tokens_generated=0))

        self._requests_served += len(prompts)
        return results

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """Embeddings not natively supported by llama-server."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Health probe via /health endpoint."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        health = await maybe_await(self._client.health)
        status = health.get("status", "error")
        if status == "ok":
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)
        if status == "loading model" or status == "no slot available":
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.degraded(reason=status, uptime_ms=uptime)
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """llama.cpp capabilities: streaming, grammar constraints, no native batching."""
        return AdapterCapabilities(
            max_context_window=self._context_size,
            supported_quantizations=[
                "gguf_q4_0", "gguf_q4_K_M", "gguf_q5_K_M", "gguf_q8_0", "gguf_f16",
            ],
            supports_streaming=True,
            supports_batching=False,
            supports_structured_output=True,  # via GBNF grammar
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=False,
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version="b4000",
        )

    async def shutdown(self) -> None:
        """Graceful shutdown."""
        self._initialized = False
        self._client = None
        logger.info("llama.cpp adapter shut down")

    # ─── llama.cpp-specific methods ───────────────────────────────────────

    async def tokenize(self, text: str) -> list[int]:
        """Tokenize text using the loaded model's tokenizer."""
        self._ensure_initialized()
        assert self._client is not None
        return await maybe_await(self._client.tokenize, text)

    async def get_slots(self) -> list[dict[str, Any]]:
        """Get current inference slot status."""
        self._ensure_initialized()
        assert self._client is not None
        return await maybe_await(self._client.slots)


