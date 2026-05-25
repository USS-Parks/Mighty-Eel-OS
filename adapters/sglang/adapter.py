"""SGLang backend adapter for MAI.

Implements RadixAttention KV cache reuse, constrained decoding
(regex, JSON schema, choice), and OpenAI-compatible streaming over
SGLang's HTTP server.

J-20 (DOUGHERTY lane) brought this adapter to the shared adapter
contract: typed error mapping, idempotent shutdown, response unwrap
for both `SglangResponse` and raw dicts, generate_batch input
validation, and an `requests_served` counter that feeds health.
"""
from __future__ import annotations

import contextlib
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
    HealthStatusKind,
    Token,
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
    maybe_await,
)
from adapters.sglang.client import SglangClient, SglangResponse, SglangStreamChunk
from adapters.sglang.config import SglangConfig


@mai_adapter(name="sglang", version="1.0.0")
class SglangAdapter(AdapterBase):
    """SGLang adapter with RadixAttention and constrained decoding."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._cfg: SglangConfig | None = None
        self._client: SglangClient | None = None
        self._model_id: str | None = None
        self._initialized_at_ms: int = 0
        self._requests_served: int = 0

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str | None:
        if config is not None:
            self._cfg = SglangConfig.from_dict(config)
        elif self._cfg is None:
            self._cfg = SglangConfig.from_dict(self._config)
        if hil_handle is not None:
            self._hil_handle = hil_handle
        if not self._cfg.host:
            raise ValidationError("sglang host must be non-empty")
        if self._cfg.port <= 0 or self._cfg.port > 65535:
            raise ValidationError(f"sglang port out of range: {self._cfg.port}")
        if self._client is None:
            self._client = SglangClient(
                base_url=self._cfg.base_url,
                timeout_ms=self._cfg.timeout_ms,
                stream_timeout_ms=self._cfg.stream_timeout_ms,
                health_check_timeout_ms=self._cfg.health_check_timeout_ms,
            )
        # Verify backend is reachable. Typed errors propagate; bare
        # socket failures become BackendUnavailableError.
        try:
            healthy = await maybe_await(self._client.health)
        except AdapterError:
            raise
        except OSError as exc:
            raise BackendUnavailableError(str(exc)) from exc
        if not healthy:
            raise BackendUnavailableError("SGLang server not reachable")
        # Discover loaded model — operator-configured default wins.
        chosen: str | None = self._cfg.default_model or None
        if chosen is None:
            try:
                models = await maybe_await(self._client.models)
            except AdapterError:
                models = []
            if models:
                head = models[0]
                chosen = head.get("id") if isinstance(head, dict) else str(head)
        self._model_id = chosen
        self._initialized_at_ms = int(time.monotonic() * 1000)
        self._initialized = True
        return self._model_id

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        self._check_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        assert self._client is not None
        assert self._cfg is not None

        kwargs = self._build_kwargs(params)

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
        except AdapterError:
            raise
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        self._requests_served += 1
        return self._result_from_response(resp)

    async def _stream_generate(
        self,
        prompt: str,
        kwargs: dict[str, Any],
    ) -> AsyncIterator[Token]:
        assert self._client is not None
        try:
            chunks = await maybe_await(
                self._client.chat_completions,
                model=self._model_id or "default",
                messages=[{"role": "user", "content": prompt}],
                stream=True,
                **kwargs,
            )
        except AdapterError:
            raise
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        index = 0
        try:
            for chunk in chunks:
                content = self._chunk_content(chunk)
                finish = self._chunk_finish_reason(chunk)
                is_end = finish is not None
                if content or is_end:
                    yield Token(
                        text=content,
                        logprob=None,
                        index=index,
                        is_end_of_text=is_end,
                    )
                    index += 1
                if is_end:
                    break
        except AdapterError:
            raise
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc
        self._requests_served += 1

    async def generate_batch(
        self,
        prompts: list[str],
        params: GenerationParams,
    ) -> list[GenerationResult]:
        self._check_initialized()
        if not prompts:
            return []
        # Sequential generation preserves input order deterministically.
        # SGLang has no public bulk endpoint; the upstream scheduler
        # benefits more from RadixAttention than from fan-out batching.
        results: list[GenerationResult] = []
        for prompt in prompts:
            result = await self.generate(prompt, params, stream=False)
            assert isinstance(result, GenerationResult)
            results.append(result)
        return results

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        self._check_initialized()
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        try:
            healthy = await maybe_await(self._client.health)
        except AdapterError:
            return HealthStatus.unavailable()
        except OSError:
            return HealthStatus.unavailable()
        uptime = max(0, int(time.monotonic() * 1000) - self._initialized_at_ms)
        if not healthy:
            return HealthStatus.degraded(
                reason="sglang health endpoint did not return ok",
                uptime_ms=uptime,
            )
        return HealthStatus(
            kind=HealthStatusKind.HEALTHY,
            uptime_ms=uptime,
            requests_served=self._requests_served,
        )

    def capabilities(self) -> AdapterCapabilities:
        cfg = self._cfg
        return AdapterCapabilities(
            supports_streaming=True,
            supports_batching=True,
            supports_embedding=False,
            supports_tool_calling=True,
            supports_structured_output=True,
            max_context_window=131072,
            supported_quantizations=["fp16", "fp8", "awq", "gptq"],
            extra={
                "radix_attention": cfg.enable_radix_attention if cfg else True,
                "constrained_decoding": True,
                "fork_parallelism": True,
                "vision": cfg.enable_vision if cfg else False,
            },
        )

    async def shutdown(self) -> None:
        # Idempotent: every member that holds backend state is dropped.
        self._initialized = False
        if self._client is not None:
            with contextlib.suppress(Exception):
                await maybe_await(self._client.close)
        self._client = None
        self._model_id = None

    # --- SGLang-specific methods ---

    async def flush_cache(self) -> bool:
        """Flush the RadixAttention prefix cache."""
        self._check_initialized()
        assert self._client is not None
        try:
            return await maybe_await(self._client.flush_cache)
        except AdapterError:
            return False

    async def get_model_info(self) -> dict[str, Any]:
        """Get detailed model info from SGLang server."""
        self._check_initialized()
        assert self._client is not None
        try:
            return await maybe_await(self._client.get_model_info)
        except AdapterError:
            return {}

    async def generate_native(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        json_schema: dict[str, Any] | None = None,
        regex: str | None = None,
    ) -> GenerationResult:
        """Use SGLang's native /generate endpoint with constrained decoding."""
        self._check_initialized()
        assert self._client is not None

        kwargs: dict[str, Any] = {}
        if params.max_tokens is not None:
            kwargs["max_tokens"] = params.max_tokens
        if params.temperature is not None:
            kwargs["temperature"] = params.temperature
        if json_schema is not None:
            kwargs["json_schema"] = json_schema
        if regex is not None:
            kwargs["regex"] = regex

        try:
            resp = await maybe_await(self._client.generate, prompt, **kwargs)
        except AdapterError:
            raise
        except TimeoutError as exc:
            raise AdapterTimeoutError(str(exc)) from exc
        except OSError as exc:
            raise BackendCrashedError(str(exc)) from exc

        self._requests_served += 1
        body = self._unwrap_body(resp)
        meta = body.get("meta_info", {}) if isinstance(body.get("meta_info"), dict) else {}
        finish = meta.get("finish_reason", "stop")
        reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
        return GenerationResult(
            text=str(body.get("text", "")),
            tokens_generated=int(meta.get("completion_tokens", 0)),
            finish_reason=reason,
        )

    # ─── helpers ──────────────────────────────────────────────────────────

    def _build_kwargs(self, params: GenerationParams) -> dict[str, Any]:
        kwargs: dict[str, Any] = {}
        if params.max_tokens is not None:
            kwargs["max_tokens"] = params.max_tokens
        if params.temperature is not None:
            kwargs["temperature"] = params.temperature
        if params.top_p is not None:
            kwargs["top_p"] = params.top_p
        if params.stop:
            kwargs["stop"] = list(params.stop)
        extra = params.extra or {}
        if "json_schema" in extra:
            kwargs["json_schema"] = extra["json_schema"]
        if "regex" in extra:
            kwargs["regex"] = extra["regex"]
        return kwargs

    @staticmethod
    def _unwrap_body(resp: Any) -> dict[str, Any]:
        """Return the response body whether `resp` is a `SglangResponse`
        dataclass or already a raw dict (the shape unit tests pass).
        """
        if isinstance(resp, SglangResponse):
            return resp.body
        if isinstance(resp, dict):
            return resp
        raise BackendCrashedError(
            f"unexpected sglang response type: {type(resp).__name__}",
        )

    def _result_from_response(self, resp: Any) -> GenerationResult:
        body = self._unwrap_body(resp)
        choices = body.get("choices") or []
        if not choices:
            raise BackendCrashedError("sglang response missing choices")
        first = choices[0] if isinstance(choices[0], dict) else {}
        message = first.get("message", {}) if isinstance(first.get("message"), dict) else {}
        usage = body.get("usage", {}) if isinstance(body.get("usage"), dict) else {}
        finish = first.get("finish_reason", "stop")
        reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
        return GenerationResult(
            text=str(message.get("content", "")),
            tokens_generated=int(usage.get("completion_tokens", 0)),
            finish_reason=reason,
        )

    @staticmethod
    def _chunk_content(chunk: Any) -> str:
        if isinstance(chunk, SglangStreamChunk):
            return chunk.content or ""
        if isinstance(chunk, dict):
            choices = chunk.get("choices") or []
            if choices and isinstance(choices[0], dict):
                delta = choices[0].get("delta", {})
                if isinstance(delta, dict):
                    return str(delta.get("content") or "")
        return ""

    @staticmethod
    def _chunk_finish_reason(chunk: Any) -> str | None:
        if isinstance(chunk, SglangStreamChunk):
            return chunk.finish_reason
        if isinstance(chunk, dict):
            choices = chunk.get("choices") or []
            if choices and isinstance(choices[0], dict):
                fr = choices[0].get("finish_reason")
                if fr is not None:
                    return str(fr)
        return None

    def _check_initialized(self) -> None:
        if not self._initialized:
            raise BackendUnavailableError("sglang adapter not initialized")
