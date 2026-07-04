"""ExLlamaV2 HTTP client.

Communicates with ExLlamaV2 server (TabbyAPI-compatible or custom).
Uses stdlib urllib only. All connections localhost-only.

"""

from __future__ import annotations

import json
import logging
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from dataclasses import dataclass
from typing import Any

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    ContextExceededError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
)

logger = logging.getLogger("mai.adapters.exllamav2.client")


@dataclass
class ExllamaResponse:
    """Parsed ExLlamaV2 API response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class ExllamaStreamChunk:
    """Single chunk from ExLlamaV2 streaming response."""

    content: str
    finish_reason: str | None = None


def _parse_response(resp: Any, started_at: float) -> ExllamaResponse:
    """Read and parse a non-streaming ExLlamaV2 response."""
    raw = resp.read().decode()
    elapsed = (time.monotonic() - started_at) * 1000
    return ExllamaResponse(
        status_code=resp.status,
        body=json.loads(raw) if raw else {},
        elapsed_ms=elapsed,
    )


def _open_response(
    req: urllib.request.Request, timeout: float, started_at: float,
) -> ExllamaResponse:
    resp = urllib.request.urlopen(req, timeout=timeout)
    try:
        return _parse_response(resp, started_at)
    finally:
        resp.close()


def _raise_url_error(error: urllib.error.URLError, timeout: float) -> None:
    if "timed out" in str(error.reason):
        raise AdapterTimeoutError(timeout_ms=int(timeout * 1000)) from error
    raise BackendUnavailableError() from error


def _stream_chunks_from_response(resp: Any) -> Iterator[ExllamaStreamChunk]:
    for line in resp:
        done, chunk = _stream_chunk_from_line(line)
        if done:
            break
        if chunk is not None:
            yield chunk


def _stream_chunk_from_line(line: bytes) -> tuple[bool, ExllamaStreamChunk | None]:
    line_str = line.decode().strip()
    if not line_str or not line_str.startswith("data: "):
        return False, None
    payload = line_str[6:]
    if payload == "[DONE]":
        return True, None
    try:
        chunk_data = json.loads(payload)
    except json.JSONDecodeError:
        return False, None
    choices = chunk_data.get("choices", [])
    if not choices:
        return False, None
    choice = choices[0]
    delta = choice.get("delta", {})
    content = delta.get("content", "")
    finish_reason = choice.get("finish_reason")
    return False, ExllamaStreamChunk(content=content, finish_reason=finish_reason)


class ExLlamaV2Client:
    """HTTP client for ExLlamaV2 server.

    Supports OpenAI-compatible endpoints as served by TabbyAPI or similar
    ExLlamaV2 wrappers. Handles multi-model loading/unloading.
    """

    def __init__(self, base_url: str, timeout_ms: int, stream_timeout_ms: int):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout_ms / 1000.0
        self._stream_timeout = stream_timeout_ms / 1000.0

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> ExllamaResponse:
        """Execute HTTP request."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode() if body else None
        headers = {"Content-Type": "application/json"} if data else {}

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        t0 = time.monotonic()
        request_timeout = timeout or self._timeout
        try:
            return _open_response(req, request_timeout, t0)
        except urllib.error.HTTPError as e:
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            raise BackendUnavailableError() from e
        except urllib.error.URLError as e:
            _raise_url_error(e, request_timeout)
            raise BackendUnavailableError() from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int(request_timeout * 1000)) from e

    def _stream_request(self, path: str, body: dict[str, Any]) -> Iterator[ExllamaStreamChunk]:
        """Execute streaming request via SSE."""
        url = f"{self._base_url}{path}"
        body["stream"] = True
        data = json.dumps(body).encode()
        headers = {"Content-Type": "application/json", "Accept": "text/event-stream"}

        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            resp = urllib.request.urlopen(req, timeout=self._stream_timeout)
        except urllib.error.HTTPError as e:
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            raise BackendUnavailableError() from e
        except (urllib.error.URLError, TimeoutError) as e:
            raise BackendUnavailableError() from e

        try:
            yield from _stream_chunks_from_response(resp)
        finally:
            resp.close()

    def _handle_http_error(self, status: int, body_text: str) -> None:
        """Map HTTP errors to MAI error types."""
        if status == 404:
            raise ModelNotFoundError(model="unknown")
        if status in (408, 504):
            raise AdapterTimeoutError(timeout_ms=int(self._timeout * 1000))
        if status == 429:
            raise RateLimitedError()
        detail = ""
        try:
            err_body = json.loads(body_text)
            detail = err_body.get("detail", err_body.get("message", ""))
        except (json.JSONDecodeError, KeyError):
            detail = body_text[:200]
        detail_lc = detail.lower()
        if "memory" in detail_lc or "oom" in detail_lc or "vram" in detail_lc:
            raise OutOfMemoryError()
        # TabbyAPI / ExLlamaV2 surface context-length violations as 400 or 422
        # with bodies that mention "context", "max_seq_len", or "too long".
        if status in (400, 413, 422) and (
            "context" in detail_lc
            or "max_seq_len" in detail_lc
            or "too long" in detail_lc
            or "exceed" in detail_lc
        ):
            raise ContextExceededError(max_context=0)
        if status >= 500:
            raise BackendUnavailableError()

    # ─── Public API ───────────────────────────────────────────────────────

    def chat_completions(
        self,
        model: str,
        messages: list[dict[str, str]],
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
    ) -> ExllamaResponse | Iterator[ExllamaStreamChunk]:
        """OpenAI-compatible chat completions."""
        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop

        if stream:
            return self._stream_request("/v1/chat/completions", body)
        return self._request("POST", "/v1/chat/completions", body)

    def models(self) -> list[dict[str, Any]]:
        """List loaded models."""
        try:
            resp = self._request("GET", "/v1/models")
            return resp.body.get("data", [])
        except (AdapterTimeoutError, BackendUnavailableError):
            return []

    def model_load(self, model_name: str, model_dir: str | None = None) -> ExllamaResponse:
        """Load a model into ExLlamaV2 server."""
        body: dict[str, Any] = {"name": model_name}
        if model_dir:
            body["model_dir"] = model_dir
        return self._request("POST", "/v1/model/load", body)

    def model_unload(self) -> ExllamaResponse:
        """Unload the current model."""
        return self._request("POST", "/v1/model/unload", {})

    def health(self) -> bool:
        """Check server health."""
        try:
            self._request("GET", "/v1/models", timeout=5.0)
            return True
        except (AdapterTimeoutError, BackendUnavailableError):
            return False
