"""Ollama adapter: full AdapterBase implementation.

Connects to a local Ollama server via HTTP REST API.
All network access is localhost-only (air-gapped by design).
GPU layer assignment via Ollama's num_gpu parameter.

Session 08 deliverable.
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
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    ModelNotFoundError,
    Token,
    UnsupportedOperationError,
    mai_adapter,
)
from adapters.ollama.client import OllamaClient, OllamaStreamChunk
from adapters.ollama.config import OllamaConfig

logger = logging.getLogger("mai.adapters.ollama")


@mai_adapter(name="ollama", version="1.0.0")
class OllamaAdapter(AdapterBase):
    """Full Ollama backend adapter.

    Supports: chat completion, raw completion, streaming, embeddings,
    model management, GPU layer assignment, health checking.

    Does NOT support: vision, tool calling, continuous batching, hot-swap.
    These are reported accurately in capabilities().
    """

    def __init__(self) -> None:
        super().__init__()
        self._client: OllamaClient | None = None
        self._config: OllamaConfig = OllamaConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model: str = ""
        self._embedding_model: str = ""
        self._available_models: list[str] = []

    async def initialize(self, config: dict[str, Any], hil_handle: Any) -> str:
        """Initialize the Ollama adapter.

        Validates connection to local Ollama server, verifies model availability.
        Returns adapter handle string.
        """
        self._config = OllamaConfig.from_dict(config) if config else OllamaConfig()
        self._client = OllamaClient(self._config)
        self._hil_handle = hil_handle
        self._start_time_ms = _now_ms()

        # Verify Ollama server is reachable
        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            raise BackendUnavailableError()

        # Discover available models
        models = await asyncio.to_thread(self._client.list_models)
        self._available_models = [m.get("name", "") for m in models]

        self._model = self._config.default_model
        self._embedding_model = self._config.embedding_model

        # Verify default model is available (warn but don't fail)
        if self._model and self._model not in self._available_models:
            # Try partial match (ollama uses tag format model:tag)
            base_name = self._model.split(":")[0]
            matched = [m for m in self._available_models if m.startswith(base_name)]
            if matched:
                logger.info(
                    f"Exact model '{self._model}' not found, "
                    f"using closest match: '{matched[0]}'",
                )
                self._model = matched[0]
            else:
                logger.warning(
                    f"Default model '{self._model}' not available locally. "
                    f"Available: {self._available_models}",
                )

        self._initialized = True
        logger.info(
            f"Ollama adapter initialized: model={self._model}, "
            f"host={self._config.host}:{self._config.port}, "
            f"models_available={len(self._available_models)}",
        )
        return f"ollama:{self._model}"

    async def generate(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from Ollama /api/generate endpoint."""
        self._ensure_initialized()
        assert self._client is not None

        options = _params_to_ollama_options(params)
        model = self._model

        # Run blocking stream in thread, yield tokens
        chunks: list[OllamaStreamChunk] = await asyncio.to_thread(
            self._collect_stream, model, prompt, options,
        )

        token_index = 0
        for chunk in chunks:
            if chunk.content:
                yield Token(
                    text=chunk.content,
                    logprob=None,
                    index=token_index,
                    is_end_of_text=chunk.done,
                )
                token_index += 1

        # Ensure we always yield an end token
        if not chunks or not chunks[-1].done:
            yield Token(text="", logprob=None, index=token_index, is_end_of_text=True)

        self._requests_served += 1

    def _collect_stream(
        self, model: str, prompt: str, options: dict[str, Any],
    ) -> list[OllamaStreamChunk]:
        """Collect streaming chunks (runs in thread)."""
        assert self._client is not None
        chunks: list[OllamaStreamChunk] = []
        stream = self._client.generate_completion(
            model=model,
            prompt=prompt,
            stream=True,
            options=options,
            keep_alive=self._config.keep_alive,
        )
        # stream is an Iterator when stream=True
        for chunk in stream:  # type: ignore[union-attr]
            chunks.append(chunk)
        return chunks

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation via sequential calls (Ollama lacks native batching)."""
        self._ensure_initialized()
        assert self._client is not None

        options = _params_to_ollama_options(params)
        results: list[GenerationResult] = []

        for prompt in prompts:
            resp = await asyncio.to_thread(
                self._generate_non_streaming, prompt, options,
            )
            results.append(resp)

        self._requests_served += 1
        return results

    def _generate_non_streaming(
        self, prompt: str, options: dict[str, Any],
    ) -> GenerationResult:
        """Single non-streaming generation (runs in thread)."""
        assert self._client is not None
        resp = self._client.generate_completion(
            model=self._model,
            prompt=prompt,
            stream=False,
            options=options,
            keep_alive=self._config.keep_alive,
        )
        # Non-streaming returns OllamaResponse
        body = resp.body  # type: ignore[union-attr]
        text = body.get("response", "")
        eval_count = body.get("eval_count", 0)

        # Determine finish reason
        done_reason = body.get("done_reason", "stop")
        if done_reason == "length":
            finish = FinishReason.MAX_TOKENS
        else:
            finish = FinishReason.STOP

        return GenerationResult(
            text=text,
            tokens_generated=eval_count,
            finish_reason=finish,
        )

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings via Ollama /api/embed endpoint."""
        self._ensure_initialized()
        assert self._client is not None

        if not self._embedding_model:
            raise UnsupportedOperationError("embedding")

        vectors = await asyncio.to_thread(
            self._client.embed, self._embedding_model, texts,
        )

        embeddings: list[Embedding] = []
        for i, vec in enumerate(vectors):
            # Ollama doesn't report input tokens per embedding,
            # estimate from text length
            estimated_tokens = max(1, len(texts[i].split()) * 4 // 3)
            embeddings.append(Embedding(vector=vec, input_tokens=estimated_tokens))

        self._requests_served += 1
        return embeddings

    async def health_check(self) -> HealthStatus:
        """Check Ollama server health."""
        if self._client is None:
            return HealthStatus.unavailable()

        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            return HealthStatus.unavailable()

        uptime_ms = _now_ms() - self._start_time_ms
        return HealthStatus.healthy(
            uptime_ms=uptime_ms,
            requests_served=self._requests_served,
        )

    def capabilities(self) -> AdapterCapabilities:
        """Report Ollama adapter capabilities."""
        return AdapterCapabilities(
            max_context_window=131072,  # Ollama supports up to 128k with some models
            supported_quantizations=["Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0", "F16"],
            supports_streaming=True,
            supports_batching=False,  # Sequential only
            supports_structured_output=False,  # Not via this adapter
            supports_vision=False,  # Deferred
            supports_tool_calling=False,  # Deferred
            supports_continuous_batching=False,
            supports_embedding=True,
            supports_hot_swap=False,
            backend_version="0.5",  # Ollama API version
        )

    async def shutdown(self) -> None:
        """Graceful shutdown. No persistent resources to release."""
        logger.info("Ollama adapter shutting down")
        self._initialized = False
        self._client = None

    # ─── Model management helpers ────────────────────────────────────────

    async def list_models(self) -> list[str]:
        """List locally available models."""
        if self._client is None:
            return []
        models = await asyncio.to_thread(self._client.list_models)
        return [m.get("name", "") for m in models]

    async def switch_model(self, model: str) -> None:
        """Switch the active model."""
        self._ensure_initialized()
        assert self._client is not None

        models = await self.list_models()
        if model not in models:
            base_name = model.split(":", maxsplit=1)[0]
            matched = [m for m in models if m.startswith(base_name)]
            if not matched:
                raise ModelNotFoundError(model=model)
            model = matched[0]

        self._model = model
        logger.info(f"Switched active model to: {model}")

    # ─── Internal helpers ────────────────────────────────────────────────

    def _ensure_initialized(self) -> None:
        """Raise if adapter is not initialized."""
        if not self._initialized:
            raise AdapterError(
                code="NotReady",
                detail="Adapter not initialized. Call initialize() first.",
            )


def _params_to_ollama_options(params: GenerationParams) -> dict[str, Any]:
    """Convert MAI GenerationParams to Ollama options dict."""
    options: dict[str, Any] = {
        "temperature": params.temperature,
        "top_p": params.top_p,
        "num_predict": params.max_tokens,
    }
    if params.stop_sequences:
        options["stop"] = params.stop_sequences
    return options


def _now_ms() -> int:
    """Current time in milliseconds."""
    return int(time.time() * 1000)
