"""Ollama HTTP client: typed wrapper around Ollama's REST API.

All requests target localhost only (air-gapped by design).
No external network access. Uses stdlib urllib to avoid
third-party HTTP library dependencies.

Session 08 deliverable.
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
    ModelNotFoundError,
    OutOfMemoryError,
)
from adapters.ollama.config import OllamaConfig

logger = logging.getLogger("mai.adapters.ollama.client")


@dataclass
class OllamaResponse:
    """Raw response from Ollama API."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class OllamaStreamChunk:
    """Single chunk from Ollama streaming response."""

    content: str
    done: bool
    model: str = ""
    total_duration: int = 0
    eval_count: int = 0
    eval_duration: int = 0


class OllamaClient:
    """HTTP client for Ollama's local REST API.

    Uses stdlib urllib - no aiohttp/httpx dependency.
    Blocking calls are intended to be run via asyncio.to_thread().
    """

    def __init__(self, config: OllamaConfig) -> None:
        self._config = config
        self._base_url = config.base_url

    def _request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        timeout_ms: int | None = None,
    ) -> OllamaResponse:
        """Make an HTTP request to Ollama."""
        url = f"{self._base_url}{path}"
        timeout_s = (timeout_ms or self._config.timeout_ms) / 1000.0

        data = None
        if body is not None:
            data = json.dumps(body).encode("utf-8")

        req = urllib.request.Request(
            url,
            data=data,
            method=method,
            headers={"Content-Type": "application/json"} if data else {},
        )

        start = time.monotonic()
        try:
            with urllib.request.urlopen(req, timeout=timeout_s) as resp:
                raw = resp.read().decode("utf-8")
                elapsed = (time.monotonic() - start) * 1000
                return OllamaResponse(
                    status_code=resp.status,
                    body=json.loads(raw) if raw else {},
                    elapsed_ms=elapsed,
                )
        except urllib.error.HTTPError as e:
            elapsed = (time.monotonic() - start) * 1000
            raw_body = e.read().decode("utf-8") if e.fp else ""
            _handle_http_error(e.code, raw_body, url)
            # _handle_http_error always raises, but mypy needs this
            raise  # pragma: no cover
        except urllib.error.URLError as e:
            if "timed out" in str(e.reason):
                raise AdapterTimeoutError(timeout_ms=int(timeout_s * 1000)) from e
            raise BackendUnavailableError() from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int(timeout_s * 1000)) from e

    def _stream_request(
        self,
        path: str,
        body: dict[str, Any],
        timeout_ms: int | None = None,
    ) -> Iterator[OllamaStreamChunk]:
        """Make a streaming POST request, yield chunks."""
        url = f"{self._base_url}{path}"
        timeout_s = (timeout_ms or self._config.stream_timeout_ms) / 1000.0

        data = json.dumps(body).encode("utf-8")
        req = urllib.request.Request(
            url,
            data=data,
            method="POST",
            headers={"Content-Type": "application/json"},
        )

        try:
            resp = urllib.request.urlopen(req, timeout=timeout_s)
        except urllib.error.HTTPError as e:
            raw_body = e.read().decode("utf-8") if e.fp else ""
            _handle_http_error(e.code, raw_body, url)
            raise  # pragma: no cover
        except urllib.error.URLError as e:
            if "timed out" in str(e.reason):
                raise AdapterTimeoutError(
                    timeout_ms=int(timeout_s * 1000),
                ) from e
            raise BackendUnavailableError() from e
        except TimeoutError as e:
            raise AdapterTimeoutError(timeout_ms=int(timeout_s * 1000)) from e

        try:
            for line in resp:
                line_str = line.decode("utf-8").strip()
                if not line_str:
                    continue
                chunk_data = json.loads(line_str)
                yield OllamaStreamChunk(
                    content=chunk_data.get("message", {}).get("content", "")
                    or chunk_data.get("response", ""),
                    done=chunk_data.get("done", False),
                    model=chunk_data.get("model", ""),
                    total_duration=chunk_data.get("total_duration", 0),
                    eval_count=chunk_data.get("eval_count", 0),
                    eval_duration=chunk_data.get("eval_duration", 0),
                )
        finally:
            resp.close()

    # ─── Public API methods ──────────────────────────────────────────────

    def generate_chat(
        self,
        model: str,
        messages: list[dict[str, str]],
        stream: bool = True,
        options: dict[str, Any] | None = None,
        keep_alive: str | None = None,
    ) -> Iterator[OllamaStreamChunk] | OllamaResponse:
        """Call /api/chat endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "messages": messages,
            "stream": stream,
        }
        if options:
            body["options"] = options
        if keep_alive is not None:
            body["keep_alive"] = keep_alive
        elif self._config.keep_alive:
            body["keep_alive"] = self._config.keep_alive

        if self._config.num_gpu_layers >= 0:
            body.setdefault("options", {})["num_gpu"] = self._config.num_gpu_layers

        if stream:
            return self._stream_request("/api/chat", body)
        return self._request("POST", "/api/chat", body)

    def generate_completion(
        self,
        model: str,
        prompt: str,
        stream: bool = True,
        options: dict[str, Any] | None = None,
        keep_alive: str | None = None,
    ) -> Iterator[OllamaStreamChunk] | OllamaResponse:
        """Call /api/generate endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "prompt": prompt,
            "stream": stream,
        }
        if options:
            body["options"] = options
        if keep_alive is not None:
            body["keep_alive"] = keep_alive
        elif self._config.keep_alive:
            body["keep_alive"] = self._config.keep_alive

        if self._config.num_gpu_layers >= 0:
            body.setdefault("options", {})["num_gpu"] = self._config.num_gpu_layers

        if stream:
            return self._stream_request("/api/generate", body)
        return self._request("POST", "/api/generate", body)

    def embed(
        self,
        model: str,
        texts: list[str],
    ) -> list[list[float]]:
        """Call /api/embed endpoint. Returns list of embedding vectors."""
        body: dict[str, Any] = {
            "model": model,
            "input": texts,
        }
        resp = self._request("POST", "/api/embed", body)
        return resp.body.get("embeddings", [])

    def list_models(self) -> list[dict[str, Any]]:
        """Call /api/tags to list locally available models."""
        resp = self._request("GET", "/api/tags")
        return resp.body.get("models", [])

    def show_model(self, model: str) -> dict[str, Any]:
        """Call /api/show for model metadata."""
        resp = self._request("POST", "/api/show", {"name": model})
        return resp.body

    def health(self) -> bool:
        """Check if Ollama server is responding.

        Probes `/api/tags` (always JSON) rather than `/` (plain text
        `Ollama is running`) — _request unconditionally json.loads the
        body, so probing `/` against a real server raises
        JSONDecodeError, which this method catches and returns False
        for, making live `health()` always-fail. Discovered during
        DOUGHERTY J-06.
        """
        try:
            resp = self._request(
                "GET", "/api/tags",
                timeout_ms=self._config.health_check_timeout_ms,
            )
            return resp.status_code == 200
        except Exception:
            return False

    def pull_model(self, model: str) -> bool:
        """Pull a model. Only works if allow_pull is True."""
        if not self._config.allow_pull:
            logger.warning(
                f"Model pull disabled (air-gapped mode). Cannot pull '{model}'",
            )
            return False
        try:
            self._request("POST", "/api/pull", {"name": model, "stream": False})
            return True
        except Exception as e:
            logger.error(f"Failed to pull model '{model}': {e}")
            return False

    def delete_model(self, model: str) -> bool:
        """Delete a local model."""
        try:
            self._request("DELETE", "/api/delete", {"name": model})
            return True
        except Exception as e:
            logger.error(f"Failed to delete model '{model}': {e}")
            return False


def _handle_http_error(status_code: int, body: str, _url: str) -> None:
    """Map Ollama HTTP errors to typed AdapterErrors."""
    detail = ""
    try:
        parsed = json.loads(body)
        detail = parsed.get("error", body)
    except json.JSONDecodeError:
        detail = body

    if status_code == 404:
        # Model not found
        raise ModelNotFoundError(model=detail)
    elif status_code == 500 and "out of memory" in detail.lower():
        raise OutOfMemoryError()
    elif status_code == 408 or status_code == 504:
        raise AdapterTimeoutError(timeout_ms=0)
    else:
        raise BackendUnavailableError()
