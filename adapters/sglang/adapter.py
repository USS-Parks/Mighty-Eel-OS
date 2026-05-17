"""SGLang backend adapter for MAI.

Implements RadixAttention KV cache reuse, constrained decoding
(regex, JSON schema, choice), fork-based parallelism, and vision support.
"""
from __future__ import annotations

import asyncio
import time
from typing import Any, AsyncIterator

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    Token,
    UnsupportedOperationError,
    mai_adapter,
)
from .client import SglangClient
from .config import SglangConfig


@mai_adapter(name="sglang", version="1.0.0")
class SglangAdapter(AdapterBase):
    """SGLang adapter with RadixAttention and constrained decoding."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._cfg: SglangConfig | None = None
        self._client: SglangClient | None = None
        self._model_id: str | None = None

    async def initialize(self) -> None:
        self._cfg = SglangConfig.from_dict(self._raw_config)
        self._client = SglangClient(
            host=self._cfg.host,
            port=self._cfg.port,
            timeout=self._cfg.timeout,
        )
        # Verify backend is reachable
        healthy = await asyncio.to_thread(self._client.health)
        if not healthy:
            raise BackendUnavailableError("SGLang server not reachable")
        # Discover loaded model
        models = await asyncio.to_thread(self._client.models)
        if models:
            self._model_id = models[0].get("id") if isinstance(models[0], dict) else models[0]
        self._initialized = True

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        self._check_initialized()
        assert self._client is not None
        assert self._cfg is not None

        # Build request kwargs
        kwargs: dict[str, Any] = {}
        if params.max_tokens is not None:
            kwargs["max_tokens"] = params.max_tokens
        if params.temperature is not None:
            kwargs["temperature"] = params.temperature
        if params.top_p is not None:
            kwargs["top_p"] = params.top_p
        if params.stop:
            kwargs["stop"] = params.stop

        # Constrained decoding from extra params
        extra = params.extra or {}
        if "json_schema" in extra:
            kwargs["json_schema"] = extra["json_schema"]
        if "regex" in extra:
            kwargs["regex"] = extra["regex"]

        if stream:
            return self._stream_generate(prompt, kwargs)

        start = time.monotonic()
        try:
            resp = await asyncio.to_thread(
                self._client.chat_completions,
                model=self._model_id or "default",
                messages=[{"role": "user", "content": prompt}],
                stream=False,
                **kwargs,
            )
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        choice = resp.get("choices", [{}])[0]
        message = choice.get("message", {})
        usage = resp.get("usage", {})

        return GenerationResult(
            text=message.get("content", ""),
            tokens_generated=usage.get("completion_tokens", 0),
            tokens_prompt=usage.get("prompt_tokens", 0),
            latency_ms=(time.monotonic() - start) * 1000,
            finish_reason=choice.get("finish_reason", "stop"),
            model_id=self._model_id or "unknown",
        )

    async def _stream_generate(
        self,
        prompt: str,
        kwargs: dict[str, Any],
    ) -> AsyncIterator[Token]:
        assert self._client is not None
        # Get streaming chunks in a thread (returns iterator)
        chunks = await asyncio.to_thread(
            self._client.chat_completions,
            model=self._model_id or "default",
            messages=[{"role": "user", "content": prompt}],
            stream=True,
            **kwargs,
        )
        for chunk in chunks:
            delta = chunk.get("choices", [{}])[0].get("delta", {})
            content = delta.get("content")
            if content:
                yield Token(text=content, logprob=None)

    async def generate_batch(
        self,
        prompts: list[str],
        params: GenerationParams,
    ) -> list[GenerationResult]:
        self._check_initialized()
        results = []
        for prompt in prompts:
            result = await self.generate(prompt, params, stream=False)
            results.append(result)
        return results

    async def embed(self, texts: list[str]) -> list[list[float]]:
        self._check_initialized()
        raise UnsupportedOperationError(
            "SGLang does not expose a dedicated embedding endpoint"
        )

    async def health_check(self) -> HealthStatus:
        if not self._initialized or self._client is None:
            return HealthStatus(
                healthy=False,
                backend_name="sglang",
                message="Not initialized",
            )
        try:
            healthy = await asyncio.to_thread(self._client.health)
            return HealthStatus(
                healthy=healthy,
                backend_name="sglang",
                model_loaded=self._model_id,
                message="OK" if healthy else "Health check failed",
            )
        except OSError as exc:
            return HealthStatus(
                healthy=False,
                backend_name="sglang",
                message=f"Connection error: {exc}",
            )

    def capabilities(self) -> AdapterCapabilities:
        return AdapterCapabilities(
            supports_streaming=True,
            supports_batching=True,
            supports_embeddings=False,
            supports_tool_calling=True,
            supports_structured_output=True,
            max_context_length=131072,
            supported_quantizations=["fp16", "fp8", "awq", "gptq"],
            extra={
                "radix_attention": self._cfg.enable_radix_attention if self._cfg else True,
                "constrained_decoding": True,
                "fork_parallelism": True,
                "vision": self._cfg.enable_vision if self._cfg else False,
            },
        )

    async def shutdown(self) -> None:
        self._initialized = False
        self._client = None

    # --- SGLang-specific methods ---

    async def flush_cache(self) -> bool:
        """Flush the RadixAttention prefix cache."""
        self._check_initialized()
        assert self._client is not None
        return await asyncio.to_thread(self._client.flush_cache)

    async def get_model_info(self) -> dict[str, Any]:
        """Get detailed model info from SGLang server."""
        self._check_initialized()
        assert self._client is not None
        return await asyncio.to_thread(self._client.get_model_info)

    async def generate_native(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        json_schema: str | None = None,
        regex: str | None = None,
    ) -> GenerationResult:
        """Use SGLang's native /generate endpoint with constrained decoding."""
        self._check_initialized()
        assert self._client is not None

        kwargs: dict[str, Any] = {}
        if params.max_tokens is not None:
            kwargs["max_new_tokens"] = params.max_tokens
        if params.temperature is not None:
            kwargs["temperature"] = params.temperature
        if json_schema:
            kwargs["json_schema"] = json_schema
        if regex:
            kwargs["regex"] = regex

        start = time.monotonic()
        try:
            resp = await asyncio.to_thread(self._client.generate, prompt, **kwargs)
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        return GenerationResult(
            text=resp.get("text", ""),
            tokens_generated=resp.get("meta_info", {}).get("completion_tokens", 0),
            tokens_prompt=resp.get("meta_info", {}).get("prompt_tokens", 0),
            latency_ms=(time.monotonic() - start) * 1000,
            finish_reason=resp.get("meta_info", {}).get("finish_reason", "stop"),
            model_id=self._model_id or "unknown",
        )

    def _check_initialized(self) -> None:
        if not self._initialized:
            raise BackendUnavailableError("Adapter not initialized")
