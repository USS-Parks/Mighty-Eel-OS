"""SGLang backend adapter for MAI.

Implements RadixAttention KV cache reuse, constrained decoding
(regex, JSON schema, choice), fork-based parallelism, and vision support.
"""
from __future__ import annotations

from collections.abc import AsyncIterator
from typing import Any

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    AdapterTimeoutError,
    BackendCrashedError,
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
from adapters.sglang.client import SglangClient
from adapters.sglang.config import SglangConfig


@mai_adapter(name="sglang", version="1.0.0")
class SglangAdapter(AdapterBase):
    """SGLang adapter with RadixAttention and constrained decoding."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._cfg: SglangConfig | None = None
        self._client: SglangClient | None = None
        self._model_id: str | None = None

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> None:
        if config is not None:
            self._cfg = SglangConfig.from_dict(config)
        elif self._cfg is None:
            self._cfg = SglangConfig.from_dict(self._config)
        if hil_handle is not None:
            self._hil_handle = hil_handle
        if self._client is None:
            self._client = SglangClient(
                host=self._cfg.host,
                port=self._cfg.port,
                timeout=self._cfg.timeout,
            )
        # Verify backend is reachable
        healthy = await maybe_await(self._client.health)
        if not healthy:
            raise BackendUnavailableError("SGLang server not reachable")
        # Discover loaded model
        models = await maybe_await(self._client.models)
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

        try:
            resp = await maybe_await(
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
        finish = choice.get("finish_reason", "stop")
        reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP

        return GenerationResult(
            text=message.get("content", ""),
            tokens_generated=usage.get("completion_tokens", 0),
            finish_reason=reason,
        )

    async def _stream_generate(
        self,
        prompt: str,
        kwargs: dict[str, Any],
    ) -> AsyncIterator[Token]:
        assert self._client is not None
        # Get streaming chunks in a thread (returns iterator)
        chunks = await maybe_await(
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

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        self._check_initialized()
        raise UnsupportedOperationError(
            "SGLang does not expose a dedicated embedding endpoint",
        )

    async def health_check(self) -> HealthStatus:
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        try:
            healthy = await maybe_await(self._client.health)
            if healthy:
                return HealthStatus.healthy(uptime_ms=0, requests_served=0)
            return HealthStatus.unavailable()
        except OSError:
            return HealthStatus.unavailable()

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
        return await maybe_await(self._client.flush_cache)

    async def get_model_info(self) -> dict[str, Any]:
        """Get detailed model info from SGLang server."""
        self._check_initialized()
        assert self._client is not None
        return await maybe_await(self._client.get_model_info)

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

        try:
            resp = await maybe_await(self._client.generate, prompt, **kwargs)
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        finish = resp.get("meta_info", {}).get("finish_reason", "stop")
        reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP

        return GenerationResult(
            text=resp.get("text", ""),
            tokens_generated=resp.get("meta_info", {}).get("completion_tokens", 0),
            finish_reason=reason,
        )

    def _check_initialized(self) -> None:
        if not self._initialized:
            raise BackendUnavailableError("Adapter not initialized")
