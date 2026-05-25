"""ONNX Runtime adapter — in-process inference for CPU/DirectML/CUDA.

This is the CPU and enterprise-Windows fallback path. It does not spawn
an external server; it loads an .onnx (or onnxruntime-genai model
directory) into the adapter subprocess.

Capability flags are determined AT initialize-time, not at import-time,
because the same package serves three legitimate model shapes:

  * onnxruntime-genai autoregressive model
        → generation + streaming, no embeddings
  * plain InferenceSession with ``embedding_only: true``
        → embeddings only, generation is UnsupportedOperationError
  * plain InferenceSession with ``embedding_only: false``
        → degraded — no generation, no embeddings; the adapter still
          initializes so health/capability surfaces work, but every
          inference path raises UnsupportedOperationError.

DOUGHERTY J-24 deliverable.
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
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    ModelNotFoundError,
    OutOfMemoryError,
    Token,
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
)
from adapters.onnxruntime.client import (
    OnnxRuntimeClient,
    OnnxRuntimeClientError,
    OnnxStreamChunk,
)
from adapters.onnxruntime.config import OnnxRuntimeConfig

logger = logging.getLogger("mai.adapters.onnxruntime")


# Map client error kinds → MAI typed error factories. Kept as a function
# so we can resolve the error variant in one place; the dict approach
# would force eager constructor calls.
def _raise_typed(kind: str, detail: str, *, model_id: str = "") -> None:
    """Translate a client-error kind into the matching MAI typed error."""
    if kind == "ValidationError":
        raise ValidationError(detail)
    if kind == "BackendUnavailable":
        raise BackendUnavailableError(detail)
    if kind == "ModelNotFound":
        raise ModelNotFoundError(model_id or detail)
    if kind == "OutOfMemory":
        raise OutOfMemoryError()
    if kind == "UnsupportedOperation":
        raise UnsupportedOperationError(detail)
    if kind == "BackendCrashed":
        raise BackendCrashedError(detail)
    # Catch-all so we never leak a raw client exception.
    raise AdapterError(code=kind or "InternalError", detail=detail)


@mai_adapter(name="onnxruntime", version="1.0.0")
class OnnxRuntimeAdapter(AdapterBase):
    """ONNX Runtime backend adapter."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._cfg: OnnxRuntimeConfig = OnnxRuntimeConfig()
        self._client: OnnxRuntimeClient | None = None
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._supports_generation: bool = False
        self._supports_embedding: bool = False
        self._backend_version: str = "unknown"
        self._model_id: str = ""

    # ── lifecycle ───────────────────────────────────────────────────────────

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Load the model in a worker thread; verify capability state.

        Returns a handle string suitable for log correlation.
        """
        if config is not None:
            self._cfg = OnnxRuntimeConfig.from_dict(config)
        if hil_handle is not None:
            self._hil_handle = hil_handle

        if not self._cfg.model_path:
            raise ValidationError(
                "OnnxRuntimeConfig.model_path is required for initialize()",
            )

        client = OnnxRuntimeClient(
            model_path=self._cfg.model_path,
            tokenizer_path=self._cfg.tokenizer_path,
            providers=list(self._cfg.providers),
        )

        try:
            info = await asyncio.to_thread(
                client.load, embedding_only=self._cfg.embedding_only,
            )
        except OnnxRuntimeClientError as exc:
            _raise_typed(exc.kind, exc.detail, model_id=self._cfg.model_path)
            return ""  # pragma: no cover — _raise_typed always raises

        self._client = client
        self._supports_generation = info.supports_generation
        self._supports_embedding = info.supports_embedding
        self._backend_version = info.backend_version
        self._model_id = info.model_id
        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            "ONNX Runtime adapter initialized: backend=%s, model=%s, "
            "providers=%s, generation=%s, embedding=%s",
            info.backend,
            info.model_id,
            self._cfg.providers,
            info.supports_generation,
            info.supports_embedding,
        )
        return f"onnxruntime:{info.backend}:{info.model_id}"

    async def shutdown(self) -> None:
        """Idempotent shutdown."""
        if self._client is not None:
            self._client.close()
        self._client = None
        self._initialized = False
        logger.info("ONNX Runtime adapter shut down")

    # ── generation ──────────────────────────────────────────────────────────

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Dual-mode: ``await`` for full result, ``async for`` for streaming."""
        self._ensure_initialized()
        if not self._supports_generation:
            raise UnsupportedOperationError("generate")
        self._validate_generate_request(prompt, params, stream=stream)

        if stream:
            return self._generate_stream(prompt, params)

        max_tokens = params.max_tokens or self._cfg.max_tokens
        temperature = params.temperature
        top_p = params.top_p

        try:
            text, count = await asyncio.wait_for(
                asyncio.to_thread(
                    self._run_generate_once,
                    prompt,
                    max_tokens,
                    temperature,
                    top_p,
                ),
                timeout=self._cfg.timeout_ms / 1000.0,
            )
        except TimeoutError as exc:  # asyncio.wait_for raises TimeoutError in 3.11+
            raise AdapterTimeoutError(self._cfg.timeout_ms) from exc
        except OnnxRuntimeClientError as exc:
            _raise_typed(exc.kind, exc.detail, model_id=self._model_id)

        finish = FinishReason.MAX_TOKENS if count >= max_tokens else FinishReason.STOP
        self._requests_served += 1
        return GenerationResult(
            text=text,
            tokens_generated=count,
            finish_reason=finish,
        )

    def _run_generate_once(
        self,
        prompt: str,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> tuple[str, int]:
        """Thread-side trampoline so OnnxRuntimeClientError travels intact."""
        assert self._client is not None
        return self._client.generate_once(
            prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
        )

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Async wrapper around the synchronous client.generate_stream()."""
        assert self._client is not None
        max_tokens = params.max_tokens or self._cfg.max_tokens
        temperature = params.temperature
        top_p = params.top_p

        try:
            chunks = await asyncio.wait_for(
                asyncio.to_thread(
                    self._collect_stream,
                    prompt,
                    max_tokens,
                    temperature,
                    top_p,
                ),
                timeout=self._cfg.stream_timeout_ms / 1000.0,
            )
        except TimeoutError as exc:
            raise AdapterTimeoutError(self._cfg.stream_timeout_ms) from exc
        except OnnxRuntimeClientError as exc:
            _raise_typed(exc.kind, exc.detail, model_id=self._model_id)
            return  # unreachable, satisfies typing

        token_index = 0
        for chunk in chunks:
            if chunk.is_final:
                yield Token(
                    text="", index=token_index, is_end_of_text=True,
                )
            else:
                yield Token(
                    text=chunk.text,
                    index=token_index,
                    is_end_of_text=False,
                )
                token_index += 1
        self._requests_served += 1

    def _collect_stream(
        self,
        prompt: str,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> list[OnnxStreamChunk]:
        """Drain the synchronous client iterator into a list (runs in thread)."""
        assert self._client is not None
        return list(
            self._client.generate_stream(
                prompt,
                max_tokens=max_tokens,
                temperature=temperature,
                top_p=top_p,
            ),
        )

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Sequential batch — onnxruntime-genai exposes no native batch API."""
        self._ensure_initialized()
        if not prompts:
            return []
        if not self._supports_generation:
            raise UnsupportedOperationError("generate_batch")

        results: list[GenerationResult] = []
        for prompt in prompts:
            result = await self.generate(prompt, params, stream=False)
            assert isinstance(result, GenerationResult)
            results.append(result)
        return results

    # ── embeddings ──────────────────────────────────────────────────────────

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Compute embeddings via the loaded InferenceSession."""
        self._ensure_initialized()
        if not self._supports_embedding:
            raise UnsupportedOperationError("embed")
        if not texts:
            return []
        self._validate_embed_request(texts)

        try:
            vectors = await asyncio.to_thread(self._run_embed, texts)
        except OnnxRuntimeClientError as exc:
            _raise_typed(exc.kind, exc.detail, model_id=self._model_id)
            return []  # unreachable

        embeddings = [
            Embedding(vector=v, input_tokens=max(1, len(t.split())))
            for v, t in zip(vectors, texts, strict=False)
        ]
        self._requests_served += 1
        return embeddings

    def _run_embed(self, texts: list[str]) -> list[list[float]]:
        assert self._client is not None
        return self._client.embed(texts)

    # ── health / capabilities ───────────────────────────────────────────────

    async def health_check(self) -> HealthStatus:
        """Cheap in-process readiness probe; no I/O."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        if not self._client.is_ready():
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.degraded(
                reason="ONNX Runtime client not ready", uptime_ms=uptime,
            )
        if not (self._supports_generation or self._supports_embedding):
            uptime = int(time.time() * 1000) - self._start_time_ms
            return HealthStatus.degraded(
                reason="loaded model exposes neither generation nor embedding",
                uptime_ms=uptime,
            )
        uptime = int(time.time() * 1000) - self._start_time_ms
        return HealthStatus.healthy(
            uptime_ms=uptime, requests_served=self._requests_served,
        )

    def capabilities(self) -> AdapterCapabilities:
        """Truthful post-initialize capability snapshot."""
        return AdapterCapabilities(
            max_context_window=self._cfg.context_window,
            supported_quantizations=["onnx_fp32", "onnx_fp16", "onnx_int8"],
            supports_streaming=self._supports_generation,
            supports_batching=False,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=False,
            supports_embedding=self._supports_embedding,
            supports_hot_swap=False,
            backend_version=self._backend_version,
            extra={
                "providers": list(self._cfg.providers),
                "model_id": self._model_id,
            },
        )

    # ── internals ───────────────────────────────────────────────────────────

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError("adapter not initialized")
