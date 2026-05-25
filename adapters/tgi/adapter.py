"""TGI (Text Generation Inference) backend adapter.

HuggingFace's production inference server with quantization (bitsandbytes,
GPTQ, AWQ), speculative decoding, watermarking for compliance audit trails,
and Flash Attention optimization.

Session 09 deliverable.
"""

from __future__ import annotations

import asyncio
import contextlib
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
    ValidationError,
    mai_adapter,
    maybe_await,
)
from adapters.tgi.client import TgiClient, TgiResponse
from adapters.tgi.config import TgiConfig

logger = logging.getLogger("mai.adapters.tgi")


@mai_adapter(name="tgi", version="1.0.0")
class TgiAdapter(AdapterBase):
    """HuggingFace Text Generation Inference adapter.

    Supports quantization configs, speculative decoding with draft models,
    watermarking for compliance, and Flash Attention. TGI serves one model
    per instance; multi-model requires multiple TGI processes.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: TgiClient | None = None
        self._config: TgiConfig = TgiConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model_id: str = ""
        self._max_input_tokens: int = 4096
        self._max_total_tokens: int = 8192

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Initialize TGI adapter. Queries /info for model metadata.

        Idempotent: if the adapter is already initialized, the live client
        is reused and a fresh handle is returned. If a reconfigure is
        requested (new ``config`` dict supplied), the prior client is torn
        down first so we never leak a session pool.
        """
        if config is not None and self._initialized and self._client is not None:
            await self.shutdown()

        if config is not None:
            self._config = TgiConfig.from_dict(config)
        elif hasattr(self, "_cfg") and self._cfg is not None:
            self._config = self._cfg

        # Config validation. TgiConfig.from_dict accepts arbitrary values, so
        # we sanity-check the bits that drive the URL/timeouts here.
        if not isinstance(self._config.host, str) or not self._config.host:
            raise ValidationError("TGI host must be a non-empty string")
        if not isinstance(self._config.port, int) or self._config.port <= 0:
            raise ValidationError("TGI port must be a positive integer")
        if self._config.timeout_ms <= 0 or self._config.stream_timeout_ms <= 0:
            raise ValidationError("TGI timeouts must be positive integers")

        if hil_handle is not None:
            self._hil_handle = hil_handle
        if self._client is None:
            self._client = TgiClient(
                base_url=self._config.base_url,
                timeout_ms=self._config.timeout_ms,
                stream_timeout_ms=self._config.stream_timeout_ms,
                health_check_timeout_ms=self._config.health_check_timeout_ms,
            )

        # Verify health
        healthy = await maybe_await(self._client.health)
        if not healthy:
            # Tear the just-built client back down so we never sit on a
            # half-initialized adapter.
            self._client = None
            raise BackendUnavailableError("TGI /health probe failed")

        # Get model info
        info = await maybe_await(self._client.info)
        if not isinstance(info, dict):
            info = {}
        self._model_id = info.get("model_id", self._config.default_model)
        self._max_input_tokens = info.get("max_input_length", self._config.max_input_tokens)
        self._max_total_tokens = info.get("max_total_tokens", self._config.max_total_tokens)

        self._start_time_ms = int(time.time() * 1000)
        self._requests_served = 0
        self._initialized = True
        logger.info(
            f"TGI adapter initialized: model={self._model_id}, "
            f"quantize={self._config.quantize}, speculate={self._config.speculate}",
        )
        return f"tgi-{self._model_id}-{self._start_time_ms}"

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError(
                "TGI adapter is not initialized; call initialize() first",
            )

    @staticmethod
    def _body_dict(resp: TgiResponse | dict[str, Any]) -> dict[str, Any]:
        """Normalize a /generate response payload into a dict.

        TGI's /generate returns a single object for a string prompt and an
        array for a list-of-strings prompt. We send a single string, so we
        only need to handle the dict case here. Anything else is treated
        as an empty body so the adapter degrades to ``text=""``
        deterministically rather than raising AttributeError.
        """
        if isinstance(resp, dict):
            return resp
        body = getattr(resp, "body", None)
        if isinstance(body, dict):
            return body
        return {}

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from TGI. Dual-mode: await for result, async-for for streaming."""
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        assert self._client is not None

        if stream:
            return self._generate_stream(prompt, params)

        # Non-streaming: return GenerationResult
        resp = await maybe_await(
            self._client.generate,
            inputs=prompt,
            max_new_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            watermark=self._config.watermark,
            stream=False,
        )
        body = self._body_dict(resp)
        generated = body.get("generated_text", "")
        details = body.get("details") or {}
        tokens_out = details.get("generated_tokens", len(generated) // 4)
        finish = details.get("finish_reason", "stop_sequence")
        reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP

        self._requests_served += 1
        return GenerationResult(text=generated, tokens_generated=tokens_out, finish_reason=reason)

    async def _generate_stream(
        self, prompt: str, params: GenerationParams,
    ) -> AsyncIterator[Token]:
        """Stream tokens from TGI.

        TGI's ``/generate_stream`` is a synchronous SSE iterator. We drive it
        from a worker thread one chunk at a time so the asyncio event loop
        keeps cooking while the HTTP read blocks, and so AdapterError
        subclasses raised mid-stream by ``TgiClient._stream_request``
        propagate to the caller as typed errors instead of being swallowed
        by ``async for``.
        """
        assert self._client is not None
        chunks_iter = self._client.generate(
            inputs=prompt,
            max_new_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
            stop=params.stop_sequences or None,
            watermark=self._config.watermark,
            stream=True,
        )

        _sentinel = object()

        def _next_chunk() -> Any:
            try:
                return next(chunks_iter)  # type: ignore[arg-type]
            except StopIteration:
                return _sentinel

        token_index = 0
        emitted_any = False
        try:
            while True:
                chunk = await asyncio.to_thread(_next_chunk)
                if chunk is _sentinel:
                    break
                is_end = chunk.finish_reason is not None or chunk.generated_text is not None
                if chunk.token_text:
                    yield Token(
                        text=chunk.token_text,
                        index=token_index,
                        is_end_of_text=is_end,
                    )
                    emitted_any = True
                    token_index += 1
                elif is_end:
                    yield Token(text="", index=token_index, is_end_of_text=True)
                    emitted_any = True
                    token_index += 1
        finally:
            close = getattr(chunks_iter, "close", None)
            if callable(close):
                with contextlib.suppress(Exception):
                    close()

        # If TGI returned zero usable chunks but did not raise, still count
        # the request as served so health metrics stay accurate.
        if emitted_any or token_index == 0:
            self._requests_served += 1

    async def generate_batch(
        self, prompts: list[str], params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batch generation via sequential calls.

        TGI batches at the server side via its continuous-batching scheduler,
        so the adapter issues independent /generate calls in order. Empty
        input is honoured (returns ``[]``). A backend failure on any prompt
        propagates as a typed AdapterError; we do not silently swallow
        per-prompt failures into placeholder results.
        """
        self._ensure_initialized()
        assert self._client is not None

        if not prompts:
            return []

        results: list[GenerationResult] = []
        for prompt in prompts:
            resp = await maybe_await(
                self._client.generate,
                inputs=prompt,
                max_new_tokens=params.max_tokens,
                temperature=params.temperature,
                top_p=params.top_p,
                stop=params.stop_sequences or None,
                watermark=self._config.watermark,
                stream=False,
            )
            body = self._body_dict(resp)
            generated = body.get("generated_text", "")
            details = body.get("details") or {}
            tokens_out = details.get("generated_tokens", len(generated) // 4)
            finish = details.get("finish_reason", "stop_sequence")
            reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
            results.append(GenerationResult(
                text=generated, tokens_generated=tokens_out, finish_reason=reason,
            ))

        self._requests_served += len(prompts)
        return results

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """TGI does not natively support embeddings."""
        raise UnsupportedOperationError("embed")

    async def health_check(self) -> HealthStatus:
        """Health probe via /health endpoint.

        TGI's /health returns 200 OK when the engine is serving and
        non-200 otherwise. We additionally consult /info: when /health
        flips false but /info still responds with a model id, we report
        DEGRADED instead of UNAVAILABLE so the scheduler can keep the
        adapter on standby rather than tearing it down outright.
        """
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()

        healthy = await maybe_await(self._client.health)
        uptime = int(time.time() * 1000) - self._start_time_ms
        if healthy:
            return HealthStatus.healthy(uptime_ms=uptime, requests_served=self._requests_served)

        info = await maybe_await(self._client.info)
        if isinstance(info, dict) and info.get("model_id"):
            return HealthStatus.degraded(
                reason="TGI /health probe failed but /info responds",
                uptime_ms=uptime,
            )
        return HealthStatus.unavailable()

    def capabilities(self) -> AdapterCapabilities:
        """TGI capabilities: streaming, quantization, speculative decoding."""
        return AdapterCapabilities(
            max_context_window=self._max_total_tokens,
            supported_quantizations=["bitsandbytes", "gptq", "awq", "eetq", "fp8"],
            supports_streaming=True,
            supports_batching=True,
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=True,
            supports_embedding=False,
            supports_hot_swap=False,
            backend_version="2.0",
        )

    async def shutdown(self) -> None:
        """Graceful, idempotent shutdown.

        Releases the HTTP client reference so the urllib pool can drain and
        clears all in-flight state. A second call is a no-op rather than an
        error, per the shared adapter lifecycle contract.
        """
        if not self._initialized and self._client is None:
            return
        self._initialized = False
        if self._client is not None:
            try:
                await maybe_await(self._client.close)
            except Exception:
                logger.debug("tgi client close failed", exc_info=True)
        self._client = None
        self._model_id = ""
        self._start_time_ms = 0
        self._requests_served = 0
        logger.info("TGI adapter shut down")
