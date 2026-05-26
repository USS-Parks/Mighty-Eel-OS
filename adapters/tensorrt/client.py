"""TensorRT-LLM HTTP client.

Communicates with NVIDIA Triton Inference Server running the TensorRT-LLM
backend over Triton's KFServing-style HTTP API. Stdlib-only; air-gapped
local-loopback by default.

Refactored under DOUGHERTY J-22 to satisfy
``docs/ADAPTER-SHARED-CONTRACT.md``:

- one urllib opener per client instance, reused across every request
  (proves pooling in unit tests and avoids per-request connection setup)
- streaming requests use the same opener
- typed error mapping for every backend failure path:
  * connect refused / DNS / socket  -> BackendUnavailableError
  * read/connect timeout            -> AdapterTimeoutError
  * HTTP 404                        -> ModelNotFoundError(<model>)
  * HTTP 408 / 504                  -> AdapterTimeoutError
  * HTTP 413 / "context"/"too long" -> ContextExceededError
  * HTTP 429                        -> RateLimitedError
  * "out of memory" / "OOM"         -> OutOfMemoryError
  * "broken pipe" / 502 after init  -> BackendCrashedError
  * everything 5xx                  -> BackendUnavailableError
  * malformed JSON                  -> BackendCrashedError
- ``close()`` releases the opener (idempotent)
- streaming yields chunks lazily and closes the underlying response in
  ``finally`` so cancellation does not leak file descriptors
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
    BackendCrashedError,
    BackendUnavailableError,
    ModelNotFoundError,
)
from adapters.tensorrt.client_helpers import (
    TritonResponse,
    TritonStreamChunk,
    iter_sse_chunks,
    map_http_error,
)

logger = logging.getLogger("mai.adapters.tensorrt.client")

class TensorRtClient:
    """HTTP client for Triton Inference Server with TensorRT-LLM backend.

    Triton exposes ``/v2/models/<model>/generate`` and
    ``/v2/models/<model>/generate_stream`` for the TensorRT-LLM backend.
    Streaming is SSE-framed (``data: {...}`` lines, terminated by
    ``[DONE]`` or by the backend's ``is_final`` flag).

    The client holds one ``urllib`` opener for its entire lifetime; every
    request -- streaming or non-streaming -- routes through that opener.
    Adapters call :meth:`close` on shutdown.
    """

    def __init__(
        self,
        base_url: str,
        timeout_ms: int,
        stream_timeout_ms: int,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout_ms / 1000.0
        self._stream_timeout = stream_timeout_ms / 1000.0
        self._opener: urllib.request.OpenerDirector | None = (
            urllib.request.build_opener()
        )
        self._closed: bool = False

    # ─── Lifecycle ────────────────────────────────────────────────────────

    @property
    def opener(self) -> urllib.request.OpenerDirector:
        """The pooled urllib opener. Stable across the client's lifetime."""
        if self._opener is None or self._closed:
            raise BackendUnavailableError(detail="client is closed")
        return self._opener

    def close(self) -> None:
        """Release the opener. Idempotent."""
        self._opener = None
        self._closed = True

    # ─── Internals ────────────────────────────────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        model_hint: str | None = None,
        timeout: float | None = None,
    ) -> TritonResponse:
        """Execute a non-streaming HTTP request against Triton."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers: dict[str, str] = (
            {"Content-Type": "application/json"} if data is not None else {}
        )

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        effective_timeout = timeout if timeout is not None else self._timeout

        t0 = time.monotonic()
        try:
            with self.opener.open(req, timeout=effective_timeout) as resp:
                raw = resp.read().decode("utf-8")
                elapsed = (time.monotonic() - t0) * 1000.0
                try:
                    parsed = json.loads(raw) if raw else {}
                except json.JSONDecodeError as exc:
                    raise BackendCrashedError(
                        detail=f"malformed JSON from Triton ({path}): {exc}",
                    ) from exc
                if not isinstance(parsed, dict):
                    raise BackendCrashedError(
                        detail=f"Triton response was not an object ({path})",
                    )
                return TritonResponse(
                    status_code=resp.status,
                    body=parsed,
                    elapsed_ms=elapsed,
                )
        except urllib.error.HTTPError as e:
            body_text = e.read().decode("utf-8", errors="replace") if e.fp else ""
            map_http_error(e.code, body_text, model_hint, effective_timeout)
            raise BackendUnavailableError() from e
        except urllib.error.URLError as e:
            reason = str(e.reason)
            if "timed out" in reason or "timeout" in reason.lower():
                raise AdapterTimeoutError(
                    timeout_ms=int(effective_timeout * 1000),
                ) from e
            raise BackendUnavailableError(detail=reason) from e
        except TimeoutError as e:
            raise AdapterTimeoutError(
                timeout_ms=int(effective_timeout * 1000),
            ) from e

    def _stream_request(
        self,
        path: str,
        body: dict[str, Any],
        *,
        model_hint: str | None = None,
    ) -> Iterator[TritonStreamChunk]:
        """Execute a streaming POST against Triton's ``generate_stream``."""
        url = f"{self._base_url}{path}"
        body = {**body, "stream": True}
        data = json.dumps(body).encode("utf-8")
        headers = {
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
        }
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")

        try:
            resp = self.opener.open(req, timeout=self._stream_timeout)
        except urllib.error.HTTPError as e:
            body_text = e.read().decode("utf-8", errors="replace") if e.fp else ""
            map_http_error(e.code, body_text, model_hint, self._stream_timeout)
            raise BackendUnavailableError() from e
        except urllib.error.URLError as e:
            reason = str(e.reason)
            if "timed out" in reason or "timeout" in reason.lower():
                raise AdapterTimeoutError(
                    timeout_ms=int(self._stream_timeout * 1000),
                ) from e
            raise BackendUnavailableError(detail=reason) from e
        except TimeoutError as e:
            raise AdapterTimeoutError(
                timeout_ms=int(self._stream_timeout * 1000),
            ) from e

        try:
            yield from iter_sse_chunks(resp)
        finally:
            resp.close()

    # ─── Public API ───────────────────────────────────────────────────────

    def generate(
        self,
        model: str,
        prompt: str,
        max_tokens: int = 512,
        temperature: float = 0.7,
        top_p: float = 0.9,
        stop: list[str] | None = None,
        *,
        stream: bool = False,
    ) -> TritonResponse | Iterator[TritonStreamChunk]:
        """Generate via Triton's TensorRT-LLM generate endpoint."""
        body: dict[str, Any] = {
            "model": model,
            "prompt": prompt,
            "max_tokens": max_tokens,
            "temperature": temperature,
            "top_p": top_p,
        }
        if stop:
            body["stop"] = stop

        if stream:
            return self._stream_request(
                f"/v2/models/{model}/generate_stream",
                body,
                model_hint=model,
            )
        return self._request(
            "POST",
            f"/v2/models/{model}/generate",
            body,
            model_hint=model,
        )

    def health(self) -> bool:
        """Probe Triton's readiness endpoint. False on any failure."""
        try:
            self._request("GET", "/v2/health/ready", timeout=self._timeout)
            return True
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return False

    def model_ready(self, model: str) -> bool:
        """Whether a specific model is ready to serve."""
        try:
            self._request(
                "GET",
                f"/v2/models/{model}/ready",
                model_hint=model,
                timeout=self._timeout,
            )
            return True
        except ModelNotFoundError:
            return False
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return False

    def model_metadata(self, model: str) -> dict[str, Any]:
        """Get a Triton model's metadata, or {} on any backend failure."""
        try:
            resp = self._request(
                "GET",
                f"/v2/models/{model}",
                model_hint=model,
                timeout=self._timeout,
            )
            return resp.body
        except (
            ModelNotFoundError,
            AdapterTimeoutError,
            BackendUnavailableError,
            BackendCrashedError,
        ):
            return {}

    def server_metadata(self) -> dict[str, Any]:
        """Triton server-level metadata, or {} on any backend failure."""
        try:
            resp = self._request("GET", "/v2", timeout=self._timeout)
            return resp.body
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return {}
