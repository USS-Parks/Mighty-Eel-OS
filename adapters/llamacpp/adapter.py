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


def _generation_result_from_body(body: dict[str, Any]) -> GenerationResult:
    choices = body.get("choices", [])
    if not choices:
        return GenerationResult(text="", tokens_generated=0)
    choice = choices[0]
    text = choice.get("message", {}).get("content", "")
    finish = choice.get("finish_reason", "stop")
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    tokens_out = body.get("usage", {}).get("completion_tokens", len(text) // 4)
    return GenerationResult(text=text, tokens_generated=tokens_out, finish_reason=reason)


def _token_from_stream_chunk(chunk: Any, index: int) -> Token | None:
    if chunk.content:
        return Token(text=chunk.content, index=index, is_end_of_text=chunk.stop)
    if chunk.stop:
        return Token(text="", index=index, is_end_of_text=True)
    return None


def _grammar_for_params(default_grammar: str | None, params: GenerationParams) -> str | None:
    if not params.structured_schema:
        return default_grammar
    return json.dumps(params.structured_schema) if not default_grammar else default_grammar


async def _stream_tokens(
    client: LlamaCppClient,
    prompt: str,
    params: GenerationParams,
    grammar: str | None,
) -> AsyncIterator[Token]:
    messages = [{"role": "user", "content": prompt}]
    chunks = await asyncio.to_thread(
        client.chat_completions,
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
        token = _token_from_stream_chunk(chunk, token_index)
        if token is None:
            continue
        yield token
        if token.text:
            token_index += 1


async def _counted_stream(owner: Any, stream: AsyncIterator[Token]) -> AsyncIterator[Token]:
    async for token in stream:
        yield token
    owner._requests_served += 1


async def _chat_completion_body(
    client: LlamaCppClient,
    prompt: str,
    params: GenerationParams,
) -> dict[str, Any]:
    messages = [{"role": "user", "content": prompt}]
    resp = await asyncio.to_thread(
        client.chat_completions,
        messages=messages,
        temperature=params.temperature,
        top_p=params.top_p,
        max_tokens=params.max_tokens,
        stop=params.stop_sequences or None,
        stream=False,
    )
    return resp.body


async def _batch_results(
    client: LlamaCppClient,
    prompts: list[str],
    params: GenerationParams,
) -> list[GenerationResult]:
    results: list[GenerationResult] = []
    for prompt in prompts:
        body = await _chat_completion_body(client, prompt, params)
        results.append(_generation_result_from_body(body))
    return results


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
        client = self._ensure_client()

        # Verify server health
        health = await maybe_await(client.health)
        if health.get("status") == "error":
            raise BackendUnavailableError()

        # Get server properties for context size
        props = await maybe_await(client.props)
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

    def _ensure_client(self) -> LlamaCppClient:
        """Return the live client, creating it once from the active config."""
        if self._client is not None:
            return self._client
        self._client = LlamaCppClient(
            base_url=self._config.base_url,
            timeout_ms=self._config.timeout_ms,
            stream_timeout_ms=self._config.stream_timeout_ms,
        )
        return self._client

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
        self._validate_generate_request(prompt, params, stream=stream)
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
        self._requests_served += 1
        return _generation_result_from_body(body)

    def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from llama-server."""
        assert self._client is not None
        grammar = _grammar_for_params(self._config.default_grammar, params)
        stream = _stream_tokens(self._client, prompt, params, grammar)
        return _counted_stream(self, stream)

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation. llama.cpp processes sequentially (no native batching)."""
        self._ensure_initialized()
        assert self._client is not None

        results = await _batch_results(self._client, prompts, params)
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
            supported_quantizations=_supported_quantizations(),
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


def _supported_quantizations() -> list[str]:
    return ["gguf_q4_0", "gguf_q4_K_M", "gguf_q5_K_M", "gguf_q8_0", "gguf_f16"]
