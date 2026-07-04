"""SGLang HTTP client.

Communicates with SGLang server's REST API. SGLang serves an
OpenAI-compatible API plus native endpoints for constrained decoding,
RadixAttention, and fork-based parallelism.

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

logger = logging.getLogger("mai.adapters.sglang.client")


@dataclass
class SglangResponse:
    """Parsed SGLang API response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class SglangStreamChunk:
    """Single chunk from SGLang streaming response."""

    content: str
    finish_reason: str | None = None
    usage: dict[str, int] | None = None


class SglangClient:
    """HTTP client for SGLang server.

    SGLang exposes OpenAI-compatible endpoints plus native endpoints
    for constrained generation and RadixAttention cache management.
    """

    def __init__(
        self,
        base_url: str,
        timeout_ms: int,
        stream_timeout_ms: int,
        *,
        health_check_timeout_ms: int = 5000,
    ):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout_ms / 1000.0
        self._stream_timeout = stream_timeout_ms / 1000.0
        self._health_timeout = health_check_timeout_ms / 1000.0
        self._opener: urllib.request.OpenerDirector | None = urllib.request.build_opener()
        self._closed = False

    @property
    def opener(self) -> urllib.request.OpenerDirector:
        if self._opener is None or self._closed:
            raise RuntimeError("SGLang client closed")
        return self._opener

    def close(self) -> None:
        self._opener = None
        self._closed = True

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> SglangResponse:
        """Execute HTTP request against SGLang server."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode() if body else None
        headers = {"Content-Type": "application/json"} if data else {}

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        t0 = time.monotonic()
        try:
            with self.opener.open(req, timeout=timeout or self._timeout) as resp:
                raw = resp.read().decode()
                elapsed = (time.monotonic() - t0) * 1000
                return SglangResponse(
                    status_code=resp.status,
                    body=json.loads(raw) if raw else {},
                    elapsed_ms=elapsed,
                )
        except urllib.error.HTTPError as e:
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            raise BackendUnavailableError() from e
        except urllib.error.URLError as e:
            if "timed out" in str(e.reason):
                raise AdapterTimeoutError(timeout_ms=int((timeout or self._timeout) * 1000)) from e
            raise BackendUnavailableError() from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int((timeout or self._timeout) * 1000)) from e

    def _stream_request(self, path: str, body: dict[str, Any]) -> Iterator[SglangStreamChunk]:
        """Execute streaming request via SSE."""
        url = f"{self._base_url}{path}"
        body["stream"] = True
        data = json.dumps(body).encode()
        headers = {"Content-Type": "application/json", "Accept": "text/event-stream"}

        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            resp = self.opener.open(req, timeout=self._stream_timeout)
        except urllib.error.HTTPError as e:
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            raise BackendUnavailableError() from e
        except (urllib.error.URLError, TimeoutError) as e:
            raise BackendUnavailableError() from e

        try:
            for line in resp:
                line_str = line.decode().strip()
                if not line_str or not line_str.startswith("data: "):
                    continue
                payload = line_str[6:]
                if payload == "[DONE]":
                    break
                try:
                    chunk_data = json.loads(payload)
                except json.JSONDecodeError:
                    continue
                choices = chunk_data.get("choices", [])
                if not choices:
                    continue
                choice = choices[0]
                delta = choice.get("delta", {})
                content = delta.get("content", "")
                finish_reason = choice.get("finish_reason")
                yield SglangStreamChunk(
                    content=content,
                    finish_reason=finish_reason,
                    usage=chunk_data.get("usage"),
                )
        finally:
            resp.close()

    def _handle_http_error(self, status: int, body_text: str) -> None:
        """Map HTTP errors to MAI error types."""
        if status == 404:
            raise ModelNotFoundError(model="unknown")
        if status == 429:
            raise RateLimitedError()
        if status in (408, 504):
            raise AdapterTimeoutError(timeout_ms=int(self._timeout * 1000))
        detail = ""
        try:
            err_body = json.loads(body_text)
            detail = err_body.get("message", err_body.get("detail", ""))
        except (json.JSONDecodeError, KeyError):
            detail = body_text[:200]
        if "memory" in detail.lower() or "oom" in detail.lower():
            raise OutOfMemoryError()
        if "context" in detail.lower() or "too long" in detail.lower():
            raise ContextExceededError(max_context=0)
        if status >= 500:
            raise BackendUnavailableError()

    # ─── Public API ──────────────────────────���────────────────────────────

    def chat_completions(
        self,
        model: str,
        messages: list[dict[str, str]],
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
        json_schema: dict[str, Any] | None = None,
        regex: str | None = None,
    ) -> SglangResponse | Iterator[SglangStreamChunk]:
        """OpenAI-compatible chat completions with SGLang extensions."""
        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if json_schema:
            body["response_format"] = {"type": "json_schema", "json_schema": json_schema}
        if regex:
            body["regex"] = regex

        if stream:
            return self._stream_request("/v1/chat/completions", body)
        return self._request("POST", "/v1/chat/completions", body)

    def generate(
        self,
        prompt: str,
        max_tokens: int = 512,
        temperature: float = 0.7,
        regex: str | None = None,
        json_schema: dict[str, Any] | None = None,
    ) -> SglangResponse:
        """SGLang native generate endpoint with constraint support."""
        body: dict[str, Any] = {
            "text": prompt,
            "sampling_params": {
                "max_new_tokens": max_tokens,
                "temperature": temperature,
            },
        }
        if regex:
            body["sampling_params"]["regex"] = regex
        if json_schema:
            body["sampling_params"]["json_schema"] = json.dumps(json_schema)
        return self._request("POST", "/generate", body)

    def models(self) -> list[dict[str, Any]]:
        """List available models."""
        try:
            resp = self._request("GET", "/v1/models")
            return resp.body.get("data", [])
        except (AdapterTimeoutError, BackendUnavailableError):
            return []

    def health(self) -> bool:
        """Check SGLang server health."""
        try:
            resp = self._request("GET", "/health", timeout=self._health_timeout)
            return resp.status_code == 200
        except (AdapterTimeoutError, BackendUnavailableError):
            return False

    def get_model_info(self) -> dict[str, Any]:
        """Get model info from SGLang."""
        try:
            resp = self._request("GET", "/get_model_info", timeout=self._health_timeout)
            return resp.body
        except (AdapterTimeoutError, BackendUnavailableError):
            return {}

    def flush_cache(self) -> bool:
        """Flush the RadixAttention cache."""
        try:
            self._request("POST", "/flush_cache", {})
            return True
        except (AdapterTimeoutError, BackendUnavailableError):
            return False
