"""llama.cpp HTTP client.

Communicates with llama-server's OpenAI-compatible REST API.
Uses stdlib urllib only (no external dependencies).
All connections are localhost-only (air-gap safe).

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
)

logger = logging.getLogger("mai.adapters.llamacpp.client")


@dataclass
class LlamaCppResponse:
    """Parsed llama-server API response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class LlamaCppStreamChunk:
    """Single chunk from llama-server streaming response."""

    content: str
    finish_reason: str | None
    stop: bool = False


def _parse_response(resp: Any, started_at: float) -> LlamaCppResponse:
    """Read and parse a non-streaming llama.cpp response."""
    raw = resp.read().decode()
    elapsed = (time.monotonic() - started_at) * 1000
    return LlamaCppResponse(
        status_code=resp.status,
        body=json.loads(raw) if raw else {},
        elapsed_ms=elapsed,
    )


def _open_response(
    req: urllib.request.Request, timeout: float, started_at: float,
) -> LlamaCppResponse:
    resp = urllib.request.urlopen(req, timeout=timeout)
    try:
        return _parse_response(resp, started_at)
    finally:
        resp.close()


def _raise_url_error(error: urllib.error.URLError, timeout: float) -> None:
    if "timed out" in str(error.reason):
        raise AdapterTimeoutError(timeout_ms=int(timeout * 1000)) from error
    raise BackendUnavailableError() from error


def _stream_chunks_from_response(resp: Any) -> Iterator[LlamaCppStreamChunk]:
    for line in resp:
        done, chunk = _stream_chunk_from_line(line)
        if done:
            break
        if chunk is not None:
            yield chunk


def _stream_chunk_from_line(line: bytes) -> tuple[bool, LlamaCppStreamChunk | None]:
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
    chunk = LlamaCppStreamChunk(
        content=content,
        finish_reason=finish_reason,
        stop=finish_reason is not None,
    )
    return False, chunk


def _slots_from_body(body: Any) -> list[dict[str, Any]]:
    if isinstance(body, list):
        return body
    return body.get("slots", [])


class LlamaCppClient:
    """HTTP client for llama-server (llama.cpp's built-in HTTP server).

    llama-server exposes an OpenAI-compatible API at /v1/chat/completions
    plus llama.cpp-specific endpoints for health, slots, and tokenization.
    """

    def __init__(self, base_url: str, timeout_ms: int, stream_timeout_ms: int):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout_ms / 1000.0
        self._stream_timeout = stream_timeout_ms / 1000.0

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> LlamaCppResponse:
        """Execute HTTP request against llama-server."""
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

    def _stream_request(
        self, path: str, body: dict[str, Any],
    ) -> Iterator[LlamaCppStreamChunk]:
        """Execute streaming request. Yields SSE chunks."""
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
        detail = ""
        try:
            err_body = json.loads(body_text)
            detail = err_body.get("message", err_body.get("error", ""))
        except (json.JSONDecodeError, KeyError):
            detail = body_text[:200]
        if "out of memory" in detail.lower() or "oom" in detail.lower():
            raise OutOfMemoryError()
        dl = detail.lower()
        if "context" in dl and ("exceed" in dl or "too long" in dl):
            raise ContextExceededError(max_context=0)
        if status >= 500:
            raise BackendUnavailableError()

    # ─── Public API ───────────────────────────────────────────────────────

    def chat_completions(
        self,
        messages: list[dict[str, str]],
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
        grammar: str | None = None,
    ) -> LlamaCppResponse | Iterator[LlamaCppStreamChunk]:
        """OpenAI-compatible chat completions."""
        body: dict[str, Any] = {
            "messages": messages,
            "temperature": temperature,
            "top_p": top_p,
            "n_predict": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if grammar:
            body["grammar"] = grammar

        if stream:
            return self._stream_request("/v1/chat/completions", body)
        return self._request("POST", "/v1/chat/completions", body)

    def completion(
        self,
        prompt: str,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
        grammar: str | None = None,
    ) -> LlamaCppResponse | Iterator[LlamaCppStreamChunk]:
        """Text completion endpoint."""
        body: dict[str, Any] = {
            "prompt": prompt,
            "temperature": temperature,
            "top_p": top_p,
            "n_predict": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if grammar:
            body["grammar"] = grammar

        if stream:
            return self._stream_request("/completion", body)
        return self._request("POST", "/completion", body)

    def health(self) -> dict[str, Any]:
        """Check llama-server health. Returns status and slot info."""
        try:
            resp = self._request("GET", "/health", timeout=5.0)
            return resp.body
        except (AdapterTimeoutError, BackendUnavailableError):
            return {"status": "error"}

    def slots(self) -> list[dict[str, Any]]:
        """Get inference slot status."""
        try:
            resp = self._request("GET", "/slots", timeout=5.0)
            return _slots_from_body(resp.body)
        except (AdapterTimeoutError, BackendUnavailableError):
            return []

    def tokenize(self, text: str) -> list[int]:
        """Tokenize text and return token IDs."""
        resp = self._request("POST", "/tokenize", {"content": text})
        return resp.body.get("tokens", [])

    def detokenize(self, tokens: list[int]) -> str:
        """Convert token IDs back to text."""
        resp = self._request("POST", "/detokenize", {"tokens": tokens})
        return resp.body.get("content", "")

    def props(self) -> dict[str, Any]:
        """Get server properties (model info, context size, etc.)."""
        try:
            resp = self._request("GET", "/props", timeout=5.0)
            return resp.body
        except (AdapterTimeoutError, BackendUnavailableError):
            return {}
