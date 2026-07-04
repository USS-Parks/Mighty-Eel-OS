"""TGI HTTP client.

Communicates with HuggingFace Text Generation Inference REST API.
Uses stdlib urllib only (no external dependencies).

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
    OutOfMemoryError,
    ValidationError,
)
from adapters.tgi.client_helpers import raise_for_http_error

logger = logging.getLogger("mai.adapters.tgi.client")


@dataclass
class TgiResponse:
    """Parsed TGI API response."""

    status_code: int
    body: dict[str, Any] | list[Any]
    elapsed_ms: float


@dataclass
class TgiStreamChunk:
    """Single chunk from TGI streaming response."""

    token_text: str
    token_id: int | None = None
    finish_reason: str | None = None
    generated_text: str | None = None


class TgiClient:
    """HTTP client for HuggingFace Text Generation Inference.

    TGI has its own API format (not OpenAI-compatible by default),
    with endpoints: /generate, /generate_stream, /info, /health.
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
            raise RuntimeError("TGI client closed")
        return self._opener

    def close(self) -> None:
        self._opener = None
        self._closed = True

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> TgiResponse:
        """Execute HTTP request against TGI server."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode() if body else None
        headers = {"Content-Type": "application/json"} if data else {}

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        t0 = time.monotonic()
        try:
            with self.opener.open(req, timeout=timeout or self._timeout) as resp:
                raw = resp.read().decode()
                elapsed = (time.monotonic() - t0) * 1000
                parsed = json.loads(raw) if raw else {}
                return TgiResponse(status_code=resp.status, body=parsed, elapsed_ms=elapsed)
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

    def _stream_request(self, path: str, body: dict[str, Any]) -> Iterator[TgiStreamChunk]:
        """Execute streaming request. TGI uses SSE with token objects."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode()
        headers = {"Content-Type": "application/json", "Accept": "text/event-stream"}

        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            resp = self.opener.open(req, timeout=self._stream_timeout)
        except urllib.error.HTTPError as e:
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            # _handle_http_error raises for all mapped codes; reaching here
            # means an unmapped 4xx — surface it as backend unavailable.
            raise BackendUnavailableError(
                f"TGI streaming connect failed (status {e.code})",
            ) from e
        except urllib.error.URLError as e:
            if "timed out" in str(getattr(e, "reason", "")):
                raise AdapterTimeoutError(timeout_ms=int(self._stream_timeout * 1000)) from e
            raise BackendUnavailableError(str(e.reason) if hasattr(e, "reason") else None) from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int(self._stream_timeout * 1000)) from e

        try:
            for line in resp:
                line_str = line.decode().strip()
                if not line_str or not line_str.startswith("data:"):
                    continue
                payload = line_str[5:].strip()
                if not payload:
                    continue
                try:
                    chunk_data = json.loads(payload)
                except json.JSONDecodeError:
                    # TGI is supposed to emit one JSON object per data: line.
                    # A malformed frame is a backend protocol violation; we
                    # raise rather than silently drop tokens.
                    raise BackendUnavailableError(
                        "TGI emitted malformed stream frame",
                    ) from None

                _raise_for_stream_error(chunk_data)
                yield _stream_chunk_from_data(chunk_data)
        finally:
            resp.close()

    def _handle_http_error(self, status: int, body_text: str) -> None:
        raise_for_http_error(status, body_text, self._timeout)

    # ─── Public API ───────────────────────────────────────────────────────

    def generate(
        self,
        inputs: str,
        max_new_tokens: int = 512,
        temperature: float = 0.7,
        top_p: float = 0.9,
        stop: list[str] | None = None,
        watermark: bool = False,
        stream: bool = False,
    ) -> TgiResponse | Iterator[TgiStreamChunk]:
        """TGI generate endpoint."""
        parameters: dict[str, Any] = {
            "max_new_tokens": max_new_tokens,
            "temperature": temperature,
            "top_p": top_p,
            "watermark": watermark,
        }
        if stop:
            parameters["stop"] = stop

        body = {"inputs": inputs, "parameters": parameters}

        if stream:
            return self._stream_request("/generate_stream", body)
        return self._request("POST", "/generate", body)

    def info(self) -> dict[str, Any]:
        """Get model info (model_id, max_tokens, quantization, etc.)."""
        try:
            resp = self._request("GET", "/info", timeout=self._health_timeout)
            return resp.body if isinstance(resp.body, dict) else {}
        except (AdapterTimeoutError, BackendUnavailableError):
            return {}

    def health(self) -> bool:
        """Check TGI server health."""
        try:
            self._request("GET", "/health", timeout=self._health_timeout)
            return True
        except (AdapterTimeoutError, BackendUnavailableError):
            return False

    def metrics(self) -> str:
        """Fetch the raw Prometheus metrics endpoint as text.

        TGI's ``/metrics`` is a text/plain Prometheus exposition; we hit it
        with a separate path so JSON decoding does not strip whitespace.
        """
        url = f"{self._base_url}/metrics"
        req = urllib.request.Request(url, method="GET")
        try:
            with self.opener.open(req, timeout=self._health_timeout) as resp:
                return resp.read().decode("utf-8", errors="replace")
        except (urllib.error.URLError, TimeoutError, urllib.error.HTTPError):
            return ""


def _raise_for_stream_error(chunk_data: Any) -> None:
    if (
        not isinstance(chunk_data, dict)
        or "error" not in chunk_data
        or "token" in chunk_data
    ):
        return

    error_obj = chunk_data.get("error")
    if isinstance(error_obj, dict):
        err_detail = str(error_obj.get("message", error_obj))
    else:
        err_detail = str(error_obj)
    err_type = str(chunk_data.get("error_type", "")).lower()
    low = err_detail.lower()
    if "memory" in low or "oom" in low:
        raise OutOfMemoryError()
    if err_type == "validation" and (
        "too long" in low or "context" in low or "max_input" in low
    ):
        raise ContextExceededError(max_context=0)
    if err_type == "validation":
        raise ValidationError(err_detail or "TGI validation error")
    raise BackendUnavailableError(err_detail or "TGI stream error")


def _stream_chunk_from_data(chunk_data: Any) -> TgiStreamChunk:
    token = chunk_data.get("token", {}) if isinstance(chunk_data, dict) else {}
    details = chunk_data.get("details") if isinstance(chunk_data, dict) else None
    return TgiStreamChunk(
        token_text=token.get("text", "") if isinstance(token, dict) else "",
        token_id=token.get("id") if isinstance(token, dict) else None,
        finish_reason=details.get("finish_reason") if isinstance(details, dict) else None,
        generated_text=chunk_data.get("generated_text")
        if isinstance(chunk_data, dict)
        else None,
    )
