"""Generic NVIDIA Triton adapter for KServe v2 workloads."""

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
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
    maybe_await,
)
from adapters.triton.adapter_helpers import build_text_input, decode_text_outputs
from adapters.triton.client import InferResponse, TritonClient
from adapters.triton.config import TritonConfig

logger = logging.getLogger("mai.adapters.triton")


@mai_adapter(name="triton", version="1.0.0")
class TritonAdapter(AdapterBase):
    """Generic NVIDIA Triton adapter (KServe v2 HTTP protocol)."""

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: TritonClient | None = None
        self._tconfig: TritonConfig = TritonConfig.from_dict(config or {})
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._model_ready: bool = False

    # ─── Lifecycle ────────────────────────────────────────────────────────

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Validate config, open the pooled client, probe Triton readiness."""
        if config is not None:
            self._tconfig = TritonConfig.from_dict(config)
        if hil_handle is not None:
            self._hil_handle = hil_handle

        if not self._tconfig.model_name:
            raise ValidationError("triton config requires model_name")

        if self._client is None:
            self._client = TritonClient(
                base_url=self._tconfig.base_url,
                timeout_ms=self._tconfig.timeout_ms,
                stream_timeout_ms=self._tconfig.stream_timeout_ms,
            )

        live = await maybe_await(self._client.server_live)
        if not live:
            # Closing the half-open client so shutdown stays idempotent
            # even if initialize() is retried.
            try:
                self._client.close()
            finally:
                self._client = None
            raise BackendUnavailableError(detail="triton server not live")

        self._model_ready = await self._poll_model_ready()
        if not self._model_ready:
            logger.warning(
                "triton model '%s' not ready; adapter degraded",
                self._tconfig.model_name,
            )

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            "triton adapter initialized: model=%s version=%s ready=%s text_io=%s",
            self._tconfig.model_name,
            self._tconfig.model_version or "latest",
            self._model_ready,
            self._tconfig.supports_text_io,
        )
        return f"triton-{self._tconfig.model_name}-{self._start_time_ms}"

    async def _poll_model_ready(self) -> bool:
        assert self._client is not None
        attempts = max(1, self._tconfig.readiness_poll_attempts)
        interval = max(0, self._tconfig.readiness_poll_interval_ms) / 1000.0
        path = self._tconfig.model_path()
        for attempt in range(attempts):
            if await maybe_await(self._client.model_ready, path):
                return True
            if attempt + 1 < attempts and interval > 0:
                await asyncio.sleep(interval)
        return False

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError(detail="triton adapter not initialized")

    # ─── Text generate / batch surface ───────────────────────────────────

    def _require_text_io(self) -> None:
        if not self._tconfig.supports_text_io:
            raise UnsupportedOperationError(
                "generate (configure input/output BYTES text tensors)",
            )

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate from Triton via the operator-declared text tensors.

        Streaming is honest: KServe v2 HTTP /infer is unary, so the
        stream surface produces a single end-of-text Token frame. The
        capabilities report ``supports_streaming=False`` accordingly --
        per the shared contract we do not pretend to stream by
        chunking a buffered response.
        """
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        self._require_text_io()
        if stream:
            return self._generate_stream(prompt, params)

        resp = await self._infer_text([prompt])
        outs = self._decode_text_outputs(resp.body)
        text = outs[0] if outs else ""
        self._requests_served += 1
        return GenerationResult(
            text=text,
            tokens_generated=max(1, len(text) // 4),
            finish_reason=FinishReason.STOP,
        )

    async def _generate_stream(
        self,
        prompt: str,
        _params: GenerationParams,
    ) -> AsyncIterator[Token]:
        resp = await self._infer_text([prompt])
        outs = self._decode_text_outputs(resp.body)
        text = outs[0] if outs else ""
        self._requests_served += 1
        yield Token(text=text, index=0, is_end_of_text=True)

    async def generate_batch(
        self,
        prompts: list[str],
        _params: GenerationParams,
    ) -> list[GenerationResult]:
        """Batched generation. Preserves input order in output order."""
        self._ensure_initialized()
        self._require_text_io()
        if not prompts:
            return []
        resp = await self._infer_text(prompts)
        outs = self._decode_text_outputs(resp.body)
        # Pad or truncate to match input length so ordering is deterministic
        # even when a backend returns a different cardinality than asked.
        if len(outs) < len(prompts):
            outs = outs + [""] * (len(prompts) - len(outs))
        elif len(outs) > len(prompts):
            outs = outs[: len(prompts)]
        self._requests_served += len(prompts)
        return [
            GenerationResult(
                text=text,
                tokens_generated=max(1, len(text) // 4),
                finish_reason=FinishReason.STOP,
            )
            for text in outs
        ]

    async def _infer_text(self, prompts: list[str]) -> InferResponse:
        assert self._client is not None
        inputs = build_text_input(self._tconfig.input_tensor_name, prompts)
        outputs = [{"name": self._tconfig.output_tensor_name}]
        result = await maybe_await(
            self._client.infer,
            self._tconfig.model_path(),
            inputs,
            outputs,
            model_hint=self._tconfig.model_name,
        )
        if not isinstance(result, InferResponse):
            raise BackendUnavailableError(
                detail="triton client.infer returned a non-InferResponse value",
            )
        return result

    def _decode_text_outputs(self, resp_body: dict[str, Any]) -> list[str]:
        return decode_text_outputs(resp_body, self._tconfig.output_tensor_name)

    # ─── Embed / raw infer surface ────────────────────────────────────────

    async def embed(self, _texts: list[str]) -> list[Embedding]:
        """Generic Triton does not have a native embedding surface.

        Embedding-style models are served via the raw ``infer()`` path
        with operator-declared tensor names. The high-level ``embed()``
        contract therefore returns ``UnsupportedOperationError``.
        """
        raise UnsupportedOperationError("embed")

    async def infer(
        self,
        inputs: list[dict[str, Any]],
        outputs: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        """Raw KServe v2 inference for non-text workloads."""
        self._ensure_initialized()
        assert self._client is not None
        result = await maybe_await(
            self._client.infer,
            self._tconfig.model_path(),
            inputs,
            outputs,
            model_hint=self._tconfig.model_name,
        )
        if not isinstance(result, InferResponse):
            raise BackendUnavailableError(
                detail="triton client.infer returned a non-InferResponse value",
            )
        self._requests_served += 1
        return result.body

    # ─── Health / Capabilities ───────────────────────────────────────────

    async def health_check(self) -> HealthStatus:
        """Lightweight health probe via /v2/health/ready and model_ready."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        if not await maybe_await(self._client.server_ready):
            return HealthStatus.unavailable()
        uptime = int(time.time() * 1000) - self._start_time_ms
        model_ready = await maybe_await(
            self._client.model_ready, self._tconfig.model_path(),
        )
        self._model_ready = model_ready
        if not model_ready:
            return HealthStatus.degraded(
                reason=f"model {self._tconfig.model_name} not ready",
                uptime_ms=uptime,
            )
        return HealthStatus.healthy(
            uptime_ms=uptime, requests_served=self._requests_served,
        )

    def capabilities(self) -> AdapterCapabilities:
        """Capabilities track what the adapter can actually do today."""
        text_io = self._tconfig.supports_text_io
        return AdapterCapabilities(
            max_context_window=self._tconfig.max_input_len if text_io else 0,
            supported_quantizations=[],
            supports_streaming=False,
            supports_batching=bool(text_io and self._tconfig.declares_batching),
            supports_structured_output=False,
            supports_vision=False,
            supports_tool_calling=False,
            supports_continuous_batching=False,
            supports_embedding=bool(self._tconfig.declares_embedding),
            supports_hot_swap=False,
            backend_version="kserve-v2",
            extra={
                "text_io": text_io,
                "model_name": self._tconfig.model_name,
                "model_version": self._tconfig.model_version or "latest",
            },
        )

    async def shutdown(self) -> None:
        """Release the pooled client. Idempotent."""
        if self._client is not None:
            try:
                self._client.close()
            except Exception:
                logger.exception("error closing triton client")
        self._client = None
        self._initialized = False
        logger.info("triton adapter shut down")
