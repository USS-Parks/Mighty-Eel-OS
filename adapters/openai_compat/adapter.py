"""Generic OpenAI-compatible local backend adapter.

Targets local OpenAI-compatible servers (LM Studio, LocalAI, FastChat,
internal gateways). All HTTP calls go through the pooled
``OpenAICompatClient``, and lifecycle, capability reporting, and error
mapping follow ``docs/ADAPTER-SHARED-CONTRACT.md``.

DOUGHERTY J-23.
"""

from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import AsyncIterator, Iterator
from typing import Any, cast

from adapters.base import (
    AdapterBase,
    AdapterCapabilities,
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    ModelNotFoundError,
    Token,
    UnsupportedOperationError,
    ValidationError,
    mai_adapter,
    maybe_await,
)
from adapters.openai_compat.client import (
    OpenAICompatClient,
    OpenAICompatResponse,
    OpenAICompatStreamChunk,
)
from adapters.openai_compat.config import OpenAICompatConfig

logger = logging.getLogger("mai.adapters.openai_compat")


@mai_adapter(name="openai_compat", version="1.0.0")
class OpenAICompatAdapter(AdapterBase):
    """Generic OpenAI-compatible local adapter.

    A single instance owns one pooled HTTP client for its lifetime.
    Capability flags are honest: ``supports_embedding`` is reported as
    configured (default False) because the underlying server may not
    implement ``/v1/embeddings``.
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        super().__init__(config)
        self._client: OpenAICompatClient | None = None
        self._config: OpenAICompatConfig = OpenAICompatConfig()
        self._start_time_ms: int = 0
        self._requests_served: int = 0
        self._known_models: list[str] = []
        self._chat_model: str = ""
        self._completion_model: str = ""
        self._embedding_model: str = ""

    # ─── Lifecycle ────────────────────────────────────────────────────

    async def initialize(
        self,
        config: dict[str, Any] | None = None,
        hil_handle: Any | None = None,
    ) -> str:
        """Validate config, build the pooled client, probe readiness."""
        if config is not None:
            self._config = OpenAICompatConfig.from_dict(config)
        if hil_handle is not None:
            self._hil_handle = hil_handle

        self._validate_config()

        if self._client is None:
            self._client = OpenAICompatClient(
                base_url=self._config.base_url,
                timeout_ms=self._config.timeout_ms,
                stream_timeout_ms=self._config.stream_timeout_ms,
                api_key=self._config.api_key,
                max_retries=self._config.max_retries,
                retry_backoff_ms=self._config.retry_backoff_ms,
            )

        # Readiness probe: GET /v1/models. A backend that does not
        # expose the endpoint still returns 200 with an empty list on
        # most local servers; either way, a network round-trip proves
        # the host is up. Failure here surfaces as a typed adapter
        # error (BackendUnavailableError or AdapterTimeoutError) from
        # the client.
        try:
            payload = await maybe_await(self._client.models)
        except BackendUnavailableError:
            # Clean up partially constructed state so retries do not
            # leak a half-initialized client.
            self._client.close()
            self._client = None
            raise

        self._known_models = _extract_model_ids(payload)
        self._chat_model = (
            self._config.chat_model
            or self._config.default_model
            or _first_or_empty(self._known_models)
        )
        self._completion_model = (
            self._config.completion_model
            or self._config.default_model
            or _first_or_empty(self._known_models)
        )
        self._embedding_model = (
            self._config.embedding_model
            or self._config.default_model
            or _first_or_empty(self._known_models)
        )

        self._start_time_ms = int(time.time() * 1000)
        self._initialized = True
        logger.info(
            "openai_compat adapter initialized: base_url=%s chat_model=%s",
            self._config.base_url,
            self._chat_model or "<unset>",
        )
        return f"openai_compat-{self._start_time_ms}"

    async def shutdown(self) -> None:
        """Idempotent shutdown: close the HTTP client once."""
        if self._client is not None:
            self._client.close()
        self._client = None
        self._initialized = False
        logger.info("openai_compat adapter shut down")

    # ─── Generation ───────────────────────────────────────────────────

    async def generate(
        self,
        prompt: str,
        params: GenerationParams,
        *,
        stream: bool = False,
    ) -> GenerationResult | AsyncIterator[Token]:
        """Generate a single completion. Streams via SSE when requested."""
        self._ensure_initialized()
        self._validate_generate_request(prompt, params, stream=stream)
        if stream:
            if not self._config.supports_streaming:
                raise UnsupportedOperationError("generate(stream=True)")
            return self._generate_stream(prompt, params)
        return await self._generate_unary(prompt, params)

    async def _generate_unary(
        self,
        prompt: str,
        params: GenerationParams,
    ) -> GenerationResult:
        assert self._client is not None
        model = self._chat_model
        if not model:
            raise ModelNotFoundError(model="")
        if self._config.prefer_endpoint == "completion":
            resp = cast(
                "OpenAICompatResponse",
                await asyncio.to_thread(
                    self._client.completion,
                    prompt=prompt,
                    model=self._completion_model or model,
                    temperature=params.temperature,
                    top_p=params.top_p,
                    max_tokens=params.max_tokens,
                    stop=list(params.stop_sequences) or None,
                    extra=dict(self._config.extra_request_fields) or None,
                ),
            )
            result = _result_from_completion(resp)
        else:
            messages = [{"role": "user", "content": prompt}]
            resp = cast(
                "OpenAICompatResponse",
                await asyncio.to_thread(
                    self._client.chat_completions,
                    messages=messages,
                    model=model,
                    temperature=params.temperature,
                    top_p=params.top_p,
                    max_tokens=params.max_tokens,
                    stop=list(params.stop_sequences) or None,
                    stream=False,
                    extra=dict(self._config.extra_request_fields) or None,
                ),
            )
            result = _result_from_chat(resp)
        self._requests_served += 1
        return result

    async def _generate_stream(
        self,
        prompt: str,
        params: GenerationParams,
    ) -> AsyncIterator[Token]:
        assert self._client is not None
        model = self._chat_model
        if not model:
            raise ModelNotFoundError(model="")
        messages = [{"role": "user", "content": prompt}]
        chunks = cast(
            "Iterator[OpenAICompatStreamChunk]",
            await asyncio.to_thread(
                self._client.chat_completions,
                messages=messages,
                model=model,
                temperature=params.temperature,
                top_p=params.top_p,
                max_tokens=params.max_tokens,
                stop=list(params.stop_sequences) or None,
                stream=True,
                extra=dict(self._config.extra_request_fields) or None,
            ),
        )
        token_index = 0
        saw_any = False
        for chunk in chunks:
            saw_any = True
            if chunk.content:
                yield Token(
                    text=chunk.content,
                    index=token_index,
                    is_end_of_text=chunk.stop,
                )
                token_index += 1
            elif chunk.stop:
                yield Token(text="", index=token_index, is_end_of_text=True)
                token_index += 1
        if not saw_any:
            # Backend ended the stream without sending any frames at
            # all. Surface a single end-of-text marker so callers do
            # not hang waiting for one.
            yield Token(text="", index=0, is_end_of_text=True)
        self._requests_served += 1

    async def generate_batch(
        self,
        prompts: list[str],
        params: GenerationParams,
    ) -> list[GenerationResult]:
        """Sequential batch via the unary path; preserves input order."""
        self._ensure_initialized()
        if not isinstance(prompts, list):
            raise ValidationError("prompts must be a list")
        if not prompts:
            return []
        results: list[GenerationResult] = []
        for prompt in prompts:
            results.append(await self._generate_unary(prompt, params))
        return results

    # ─── Embeddings ───────────────────────────────────────────────────

    async def embed(self, texts: list[str]) -> list[Embedding]:
        """Embeddings via ``/v1/embeddings`` when the backend supports it."""
        self._ensure_initialized()
        if not self._config.supports_embeddings:
            raise UnsupportedOperationError("embed")
        assert self._client is not None
        if not texts:
            return []
        self._validate_embed_request(texts)
        model = self._embedding_model
        if not model:
            raise ModelNotFoundError(model="")
        resp = await asyncio.to_thread(
            self._client.embeddings,
            input_texts=texts,
            model=model,
        )
        return _embeddings_from_response(resp, len(texts))

    # ─── Health ───────────────────────────────────────────────────────

    async def health_check(self) -> HealthStatus:
        """Lightweight readiness probe via ``GET /v1/models``."""
        if not self._initialized or self._client is None:
            return HealthStatus.unavailable()
        try:
            payload = await asyncio.to_thread(self._client.models)
        except (BackendUnavailableError, ModelNotFoundError):
            return HealthStatus.unavailable()
        except Exception:
            logger.warning("openai_compat health probe failed", exc_info=True)
            return HealthStatus.unavailable()
        uptime = int(time.time() * 1000) - self._start_time_ms
        models = _extract_model_ids(payload)
        if not models and not self._known_models:
            return HealthStatus.degraded(
                reason="backend reachable but exposes no models",
                uptime_ms=uptime,
            )
        return HealthStatus.healthy(
            uptime_ms=uptime,
            requests_served=self._requests_served,
        )

    # ─── Capabilities ─────────────────────────────────────────────────

    def capabilities(self) -> AdapterCapabilities:
        """Report only what this adapter actually implements."""
        return AdapterCapabilities(
            max_context_window=self._config.context_size,
            supported_quantizations=[],
            supports_streaming=self._config.supports_streaming,
            supports_batching=False,
            supports_structured_output=self._config.supports_structured_output,
            supports_vision=False,
            supports_tool_calling=self._config.supports_tool_calling,
            supports_continuous_batching=False,
            supports_embedding=self._config.supports_embeddings,
            supports_hot_swap=False,
            backend_version=self._config.backend_version,
        )

    # ─── Internals ────────────────────────────────────────────────────

    def _ensure_initialized(self) -> None:
        if not self._initialized or self._client is None:
            raise BackendUnavailableError("adapter not initialized")

    def _validate_config(self) -> None:
        cfg = self._config
        if cfg.scheme not in ("http", "https"):
            raise ValidationError(f"invalid scheme: {cfg.scheme!r}")
        if not cfg.host:
            raise ValidationError("host must be set")
        if not (0 < cfg.port < 65536):
            raise ValidationError(f"port out of range: {cfg.port}")
        if cfg.prefer_endpoint not in ("chat", "completion"):
            raise ValidationError(
                f"prefer_endpoint must be 'chat' or 'completion', got "
                f"{cfg.prefer_endpoint!r}",
            )
        if cfg.timeout_ms <= 0 or cfg.stream_timeout_ms <= 0:
            raise ValidationError("timeouts must be positive")
        if cfg.max_retries < 0:
            raise ValidationError("max_retries must be >= 0")


# ─── Response helpers ─────────────────────────────────────────────────


def _extract_model_ids(payload: dict[str, Any]) -> list[str]:
    """Pull a list of model ids out of ``GET /v1/models`` shapes.

    Tolerates both ``{"data": [{"id": "x"}, ...]}`` (OpenAI canonical)
    and ``{"models": ["x", ...]}`` (some local servers).
    """
    if not isinstance(payload, dict):
        return []
    data = payload.get("data")
    if isinstance(data, list):
        ids = [str(m.get("id")) for m in data if isinstance(m, dict) and m.get("id")]
        if ids:
            return ids
    models = payload.get("models")
    if isinstance(models, list):
        return [str(m) for m in models if m]
    return []


def _first_or_empty(items: list[str]) -> str:
    return items[0] if items else ""


def _result_from_chat(resp: OpenAICompatResponse) -> GenerationResult:
    """Map a unary chat-completion response to a GenerationResult."""
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    choices = body.get("choices") or []
    if not choices:
        return GenerationResult(text="", tokens_generated=0, finish_reason=FinishReason.STOP)
    choice = choices[0]
    message = choice.get("message") or {}
    text = str(message.get("content") or "")
    finish = str(choice.get("finish_reason") or "stop")
    usage = body.get("usage") or {}
    tokens_out = int(usage.get("completion_tokens") or max(len(text) // 4, 1 if text else 0))
    return GenerationResult(
        text=text,
        tokens_generated=tokens_out,
        finish_reason=_finish_from_str(finish),
    )


def _result_from_completion(resp: OpenAICompatResponse) -> GenerationResult:
    """Map a unary text-completion response to a GenerationResult."""
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    choices = body.get("choices") or []
    if not choices:
        return GenerationResult(text="", tokens_generated=0, finish_reason=FinishReason.STOP)
    choice = choices[0]
    text = str(choice.get("text") or "")
    finish = str(choice.get("finish_reason") or "stop")
    usage = body.get("usage") or {}
    tokens_out = int(usage.get("completion_tokens") or max(len(text) // 4, 1 if text else 0))
    return GenerationResult(
        text=text,
        tokens_generated=tokens_out,
        finish_reason=_finish_from_str(finish),
    )


def _finish_from_str(value: str) -> FinishReason:
    v = value.lower()
    if v == "length":
        return FinishReason.MAX_TOKENS
    if v == "stop_sequence":
        return FinishReason.STOP_SEQUENCE
    return FinishReason.STOP


def _embeddings_from_response(
    resp: OpenAICompatResponse,
    expected: int,
) -> list[Embedding]:
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    data = body.get("data") if isinstance(body, dict) else None
    if not isinstance(data, list):
        raise ValidationError("embeddings response missing 'data' list")
    usage = body.get("usage") if isinstance(body, dict) else None
    total_prompt_tokens = 0
    if isinstance(usage, dict):
        total_prompt_tokens = int(usage.get("prompt_tokens") or 0)
    per_input_tokens = (
        total_prompt_tokens // expected if expected and total_prompt_tokens else 0
    )

    # Preserve OpenAI's "index" ordering when the backend supplies it;
    # fall back to list position when it does not.
    indexed: list[tuple[int, Embedding]] = []
    for fallback_idx, entry in enumerate(data):
        if not isinstance(entry, dict):
            raise ValidationError("embedding entry must be an object")
        raw_vector = entry.get("embedding")
        if not isinstance(raw_vector, list) or not raw_vector:
            raise ValidationError("embedding entry missing non-empty vector")
        vector = [float(x) for x in raw_vector]
        idx = entry.get("index")
        idx_int = int(idx) if isinstance(idx, int) else fallback_idx
        indexed.append((idx_int, Embedding(vector=vector, input_tokens=per_input_tokens)))
    indexed.sort(key=lambda pair: pair[0])
    return [emb for _, emb in indexed]


# Keep names importable even if linters miss the indirect use through
# ``_stream_request`` Iterator type hints.
__all__ = [
    "OpenAICompatAdapter",
    "OpenAICompatClient",
    "OpenAICompatConfig",
    "OpenAICompatResponse",
    "OpenAICompatStreamChunk",
]
