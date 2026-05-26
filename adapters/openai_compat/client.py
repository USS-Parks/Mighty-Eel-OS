"""HTTP client for a generic OpenAI-compatible local server.

Speaks the standard ``/v1/models``, ``/v1/completions``,
``/v1/chat/completions``, and ``/v1/embeddings`` surface. Stdlib only
(``urllib.request``) so the adapter inherits no new wheel weight and
keeps the air-gap policy intact.

DOUGHERTY J-23.
"""

from __future__ import annotations

import json
import logging
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from typing import Any

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    ContextExceededError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    ValidationError,
)
from adapters.openai_compat.http_helpers import (
    OpenAICompatResponse,
    OpenAICompatStreamChunk,
    error_detail,
    extract_model,
    is_context_error,
    is_oom_error,
    read_http_error_body,
    stream_chunk_from_payload,
)

logger = logging.getLogger("mai.adapters.openai_compat.client")

class OpenAICompatClient:
    """Pooled HTTP client for a generic OpenAI-compatible local backend.

    A single client instance owns one persistent ``urllib`` opener and
    is reused for the full lifetime of the adapter, matching the
    "one client/session pool per initialized adapter instance" rule in
    ``docs/ADAPTER-SHARED-CONTRACT.md`` §HTTP And Session Pooling.
    """

    def __init__(
        self,
        base_url: str,
        timeout_ms: int,
        stream_timeout_ms: int,
        api_key: str | None = None,
        max_retries: int = 0,
        retry_backoff_ms: int = 100,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = max(timeout_ms, 1) / 1000.0
        self._stream_timeout = max(stream_timeout_ms, 1) / 1000.0
        self._api_key = api_key
        self._max_retries = max(0, max_retries)
        self._retry_backoff = max(0, retry_backoff_ms) / 1000.0
        # Persistent opener — reused across every request and stream.
        self._opener: urllib.request.OpenerDirector | None = (
            urllib.request.build_opener()
        )
        self._closed: bool = False

    # ─── Lifecycle ────────────────────────────────────────────────────

    @property
    def closed(self) -> bool:
        return self._closed

    def close(self) -> None:
        """Drop the persistent opener. Idempotent."""
        self._opener = None
        self._closed = True

    # ─── Public API ───────────────────────────────────────────────────

    def models(self) -> dict[str, Any]:
        """List models on the backend. Used for readiness checks."""
        return self._request("GET", "/v1/models").body

    def completion(
        self,
        prompt: str,
        model: str,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        extra: dict[str, Any] | None = None,
    ) -> OpenAICompatResponse:
        """Legacy text-completion endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "prompt": prompt,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if extra:
            body.update(extra)
        return self._request("POST", "/v1/completions", body)

    def chat_completions(
        self,
        messages: list[dict[str, str]],
        model: str,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
        extra: dict[str, Any] | None = None,
    ) -> OpenAICompatResponse | Iterator[OpenAICompatStreamChunk]:
        """OpenAI-compatible chat completion (with optional SSE stream)."""
        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if extra:
            body.update(extra)
        if stream:
            return self._stream_request("/v1/chat/completions", body)
        return self._request("POST", "/v1/chat/completions", body)

    def embeddings(
        self,
        input_texts: list[str],
        model: str,
    ) -> OpenAICompatResponse:
        """OpenAI-compatible embeddings call."""
        body = {"model": model, "input": input_texts}
        return self._request("POST", "/v1/embeddings", body)

    # ─── Internals ────────────────────────────────────────────────────

    def _headers(self, *, accept_sse: bool = False) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if accept_sse:
            headers["Accept"] = "text/event-stream"
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        return headers

    def _request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> OpenAICompatResponse:
        """Single unary request with bounded retry on transient failures."""
        if self._closed or self._opener is None:
            raise BackendUnavailableError("client closed")

        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode() if body is not None else None
        headers = self._headers() if data else (
            {"Authorization": f"Bearer {self._api_key}"} if self._api_key else {}
        )

        deadline = timeout if timeout is not None else self._timeout
        attempts = self._max_retries + 1
        last_exc: Exception | None = None
        for attempt in range(attempts):
            req = urllib.request.Request(
                url, data=data, headers=headers, method=method,
            )
            t0 = time.monotonic()
            try:
                with self._opener.open(req, timeout=deadline) as resp:
                    raw = resp.read().decode("utf-8") if resp.length != 0 else ""
                    elapsed = (time.monotonic() - t0) * 1000.0
                    parsed: dict[str, Any] = {}
                    if raw:
                        try:
                            parsed = json.loads(raw)
                        except json.JSONDecodeError as e:
                            raise ValidationError(
                                f"backend returned non-JSON body: {e}",
                            ) from e
                    return OpenAICompatResponse(
                        status_code=resp.status,
                        body=parsed,
                        elapsed_ms=elapsed,
                    )
            except urllib.error.HTTPError as e:
                body_text = read_http_error_body(e)
                self._handle_http_error(e.code, body_text)
                # _handle_http_error only returns on 5xx with no retry decision.
                if e.code >= 500 and attempt < attempts - 1:
                    last_exc = e
                    time.sleep(self._retry_backoff)
                    continue
                raise BackendUnavailableError(
                    f"HTTP {e.code} from {path}: {body_text[:200]}",
                ) from e
            except urllib.error.URLError as e:
                reason = str(getattr(e, "reason", ""))
                if "timed out" in reason.lower():
                    raise AdapterTimeoutError(
                        timeout_ms=int(deadline * 1000),
                    ) from e
                if attempt < attempts - 1:
                    last_exc = e
                    time.sleep(self._retry_backoff)
                    continue
                raise BackendUnavailableError(reason or "connection failed") from e
            except TimeoutError as e:
                raise AdapterTimeoutError(timeout_ms=int(deadline * 1000)) from e
        # All retries exhausted on a recoverable failure path.
        raise BackendUnavailableError(
            f"backend unreachable after {attempts} attempts: {last_exc}",
        )

    def _stream_request(
        self,
        path: str,
        body: dict[str, Any],
    ) -> Iterator[OpenAICompatStreamChunk]:
        """SSE streaming request. Single shot; no retries."""
        if self._closed or self._opener is None:
            raise BackendUnavailableError("client closed")
        url = f"{self._base_url}{path}"
        body = dict(body)
        body["stream"] = True
        data = json.dumps(body).encode()
        headers = self._headers(accept_sse=True)
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            resp = self._opener.open(req, timeout=self._stream_timeout)
        except urllib.error.HTTPError as e:
            body_text = read_http_error_body(e)
            self._handle_http_error(e.code, body_text)
            raise BackendUnavailableError(
                f"HTTP {e.code} on stream open: {body_text[:200]}",
            ) from e
        except urllib.error.URLError as e:
            reason = str(getattr(e, "reason", ""))
            if "timed out" in reason.lower():
                raise AdapterTimeoutError(
                    timeout_ms=int(self._stream_timeout * 1000),
                ) from e
            raise BackendUnavailableError(reason or "stream connect failed") from e

        try:
            for line in resp:
                line_str = line.decode("utf-8", errors="replace").strip()
                if not line_str or line_str.startswith(":"):
                    continue
                if not line_str.startswith("data:"):
                    continue
                payload = line_str[len("data:") :].strip()
                if payload == "[DONE]":
                    break
                chunk = stream_chunk_from_payload(payload)
                if chunk is not None:
                    yield chunk
        finally:
            try:
                resp.close()
            except Exception:
                logger.debug("stream close raised; ignoring", exc_info=True)

    def _handle_http_error(self, status: int, body_text: str) -> None:
        """Map an HTTP error response to a typed MAI adapter error.

        Returns instead of raising only for 5xx codes so the caller can
        decide whether to retry; all other codes raise here.
        """
        if status == 429:
            raise RateLimitedError()
        if status in (408, 504):
            raise AdapterTimeoutError(timeout_ms=int(self._timeout * 1000))
        detail = error_detail(body_text)
        detail_l = detail.lower()
        if status == 404:
            # OpenAI-style "model_not_found" or generic missing route.
            if "model" in detail_l or not detail:
                raise ModelNotFoundError(model=extract_model(detail) or "unknown")
            raise BackendUnavailableError(f"HTTP 404: {detail[:200]}")
        if status == 400:
            if is_context_error(detail_l):
                raise ContextExceededError(max_context=0)
            if is_oom_error(detail_l):
                raise OutOfMemoryError()
            raise ValidationError(detail or "invalid request")
        if status == 401 or status == 403:
            raise ValidationError(detail or f"auth rejected ({status})")
        if status == 422:
            raise ValidationError(detail or "unprocessable entity")
        if status >= 500:
            if is_oom_error(detail_l):
                raise OutOfMemoryError()
            # 5xx falls through to the caller for retry/raise decision.
            return
        raise BackendUnavailableError(f"unexpected HTTP {status}: {detail[:200]}")
