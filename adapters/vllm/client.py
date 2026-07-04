"""vLLM HTTP client.

Communicates with vLLM's OpenAI-compatible REST API.
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
    RateLimitedError,
)

logger = logging.getLogger("mai.adapters.vllm.client")


@dataclass
class VllmResponse:
    """Parsed vLLM API response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class VllmStreamChunk:
    """Single chunk from vLLM streaming response (SSE format)."""

    content: str
    finish_reason: str | None
    model: str
    usage: dict[str, int] | None = None


class VllmClient:
    """HTTP client for vLLM's OpenAI-compatible API.

    vLLM natively serves the OpenAI chat/completions format, so this client
    follows that contract. All calls are blocking (wrapped in asyncio.to_thread
    by the adapter layer).
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
            raise RuntimeError("vLLM client closed")
        return self._opener

    def close(self) -> None:
        self._opener = None
        self._closed = True

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> VllmResponse:
        """Execute HTTP request against vLLM server."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode() if body else None
        headers = {"Content-Type": "application/json"} if data else {}

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        t0 = time.monotonic()
        try:
            with self.opener.open(req, timeout=timeout or self._timeout) as resp:
                raw = resp.read().decode()
                elapsed = (time.monotonic() - t0) * 1000
                return VllmResponse(
                    status_code=resp.status,
                    body=json.loads(raw) if raw else {},
                    elapsed_ms=elapsed,
                )
        except urllib.error.HTTPError as e:
            elapsed = (time.monotonic() - t0) * 1000
            body_text = e.read().decode() if e.fp else ""
            self._handle_http_error(e.code, body_text)
            # Fallback (shouldn't reach here)
            raise BackendUnavailableError() from e
        except urllib.error.URLError as e:
            if "timed out" in str(e.reason):
                raise AdapterTimeoutError(timeout_ms=int((timeout or self._timeout) * 1000)) from e
            raise BackendUnavailableError() from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int((timeout or self._timeout) * 1000)) from e

    def _stream_request(
        self, path: str, body: dict[str, Any],
    ) -> Iterator[VllmStreamChunk]:
        """Execute streaming request. Yields SSE chunks."""
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
                yield VllmStreamChunk(
                    content=content,
                    finish_reason=finish_reason,
                    model=chunk_data.get("model", ""),
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
        # Parse error body for details
        detail = ""
        try:
            err_body = json.loads(body_text)
            detail = err_body.get("message", err_body.get("detail", ""))
        except (json.JSONDecodeError, KeyError):
            detail = body_text[:200]
        if "out of memory" in detail.lower() or "oom" in detail.lower():
            raise OutOfMemoryError()
        if "context length" in detail.lower() or "too long" in detail.lower():
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
        response_format: dict[str, Any] | None = None,
        guided_json: dict[str, Any] | None = None,
        guided_regex: str | None = None,
    ) -> VllmResponse | Iterator[VllmStreamChunk]:
        """OpenAI-compatible chat completions endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop
        if response_format:
            body["response_format"] = response_format
        if guided_json:
            body["guided_json"] = guided_json
        if guided_regex:
            body["guided_regex"] = guided_regex

        if stream:
            return self._stream_request("/v1/chat/completions", body)
        return self._request("POST", "/v1/chat/completions", body)

    def completions(
        self,
        model: str,
        prompt: str,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 512,
        stop: list[str] | None = None,
        stream: bool = False,
    ) -> VllmResponse | Iterator[VllmStreamChunk]:
        """OpenAI-compatible completions endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "prompt": prompt,
            "temperature": temperature,
            "top_p": top_p,
            "max_tokens": max_tokens,
        }
        if stop:
            body["stop"] = stop

        if stream:
            return self._stream_request("/v1/completions", body)
        return self._request("POST", "/v1/completions", body)

    def models(self) -> list[dict[str, Any]]:
        """List available models (OpenAI /v1/models format)."""
        resp = self._request("GET", "/v1/models")
        return resp.body.get("data", [])

    def health(self) -> bool:
        """Check vLLM server health."""
        try:
            self._request("GET", "/health", timeout=self._health_timeout)
            return True
        except (AdapterTimeoutError, BackendUnavailableError):
            return False

    def metrics(self) -> dict[str, Any]:
        """Fetch vLLM Prometheus metrics endpoint (parsed as text)."""
        try:
            resp = self._request("GET", "/metrics", timeout=self._health_timeout)
            return resp.body
        except (AdapterTimeoutError, BackendUnavailableError):
            return {}

    def lora_load(self, lora_name: str, lora_path: str) -> VllmResponse:
        """Load a LoRA adapter at runtime."""
        body = {"lora_name": lora_name, "lora_path": lora_path}
        return self._request("POST", "/v1/load_lora", body)

    def lora_unload(self, lora_name: str) -> VllmResponse:
        """Unload a LoRA adapter."""
        body = {"lora_name": lora_name}
        return self._request("POST", "/v1/unload_lora", body)
