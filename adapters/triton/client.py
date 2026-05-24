"""Generic Triton (KServe v2) HTTP client.

Implements just enough of the KServe v2 inference protocol to support
generic model inference. Distinct from the TensorRT-LLM client in
``adapters/tensorrt/client.py``.

Endpoints used:
- ``GET  /v2/health/live``
- ``GET  /v2/health/ready``
- ``GET  /v2/models/<model>[/versions/<v>]``
- ``GET  /v2/models/<model>[/versions/<v>]/ready``
- ``POST /v2/models/<model>[/versions/<v>]/infer``

Per ``docs/ADAPTER-SHARED-CONTRACT.md``:

- one urllib opener per client instance, reused across every request
  (pooling proved by unit tests)
- typed error mapping at the HTTP boundary
- ``close()`` releases the opener and is idempotent
- stdlib-only

J-26 deliverable.
"""

from __future__ import annotations

import json
import logging
import re
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

from adapters.base import (
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    ContextExceededError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    ValidationError,
)

logger = logging.getLogger("mai.adapters.triton.client")

# Word-boundary OOM detector. Avoids matching e.g. ``boom`` or ``room``.
_OOM_PATTERN = re.compile(r"(?:\boom\b|out[ _]of[ _]memory)", re.IGNORECASE)


@dataclass
class InferResponse:
    """Parsed KServe v2 inference response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


class TritonClient:
    """KServe v2 HTTP client. One pooled opener per instance."""

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
        self._closed = False

    # ─── Lifecycle ────────────────────────────────────────────────────────

    @property
    def opener(self) -> urllib.request.OpenerDirector:
        """The pooled urllib opener. Stable across the client's lifetime."""
        if self._opener is None or self._closed:
            raise BackendUnavailableError(detail="triton client is closed")
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
    ) -> InferResponse:
        """Execute a non-streaming HTTP request against Triton."""
        url = f"{self._base_url}{path}"
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers: dict[str, str] = (
            {"Content-Type": "application/json"} if data is not None else {}
        )
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        eff_timeout = timeout if timeout is not None else self._timeout

        t0 = time.monotonic()
        try:
            with self.opener.open(req, timeout=eff_timeout) as resp:
                raw = resp.read().decode("utf-8")
                elapsed = (time.monotonic() - t0) * 1000.0
                if not raw:
                    parsed: dict[str, Any] = {}
                else:
                    try:
                        decoded = json.loads(raw)
                    except json.JSONDecodeError as exc:
                        raise BackendCrashedError(
                            detail=f"malformed JSON from Triton ({path}): {exc}",
                        ) from exc
                    if not isinstance(decoded, dict):
                        raise BackendCrashedError(
                            detail=f"Triton response was not an object ({path})",
                        )
                    parsed = decoded
                return InferResponse(
                    status_code=resp.status, body=parsed, elapsed_ms=elapsed,
                )
        except urllib.error.HTTPError as e:
            body_text = e.read().decode("utf-8", errors="replace") if e.fp else ""
            _map_http_error(e.code, body_text, model_hint, eff_timeout)
            raise BackendUnavailableError() from e  # unreachable; mypy needs it
        except urllib.error.URLError as e:
            reason = str(e.reason)
            if "timed out" in reason or "timeout" in reason.lower():
                raise AdapterTimeoutError(
                    timeout_ms=int(eff_timeout * 1000),
                ) from e
            raise BackendUnavailableError(detail=reason) from e
        except TimeoutError as e:
            raise AdapterTimeoutError(
                timeout_ms=int(eff_timeout * 1000),
            ) from e

    # ─── Public API ───────────────────────────────────────────────────────

    def server_live(self) -> bool:
        """Probe Triton's liveness endpoint. False on any failure."""
        try:
            self._request("GET", "/v2/health/live", timeout=self._timeout)
            return True
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return False

    def server_ready(self) -> bool:
        """Probe Triton's readiness endpoint. False on any failure."""
        try:
            self._request("GET", "/v2/health/ready", timeout=self._timeout)
            return True
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return False

    def model_ready(self, model_path: str) -> bool:
        """Whether a specific model (and optional version) is ready to serve."""
        try:
            self._request("GET", f"{model_path}/ready", timeout=self._timeout)
            return True
        except ModelNotFoundError:
            return False
        except (AdapterTimeoutError, BackendUnavailableError, BackendCrashedError):
            return False

    def model_metadata(self, model_path: str) -> dict[str, Any]:
        """Get a Triton model's metadata, or {} on any backend failure."""
        try:
            resp = self._request("GET", model_path, timeout=self._timeout)
            return resp.body
        except (
            ModelNotFoundError,
            AdapterTimeoutError,
            BackendUnavailableError,
            BackendCrashedError,
        ):
            return {}

    def infer(
        self,
        model_path: str,
        inputs: list[dict[str, Any]],
        outputs: list[dict[str, Any]] | None = None,
        *,
        model_hint: str | None = None,
    ) -> InferResponse:
        """Issue a KServe v2 inference request.

        ``inputs`` is a list of KServe input-tensor dicts (each with
        ``name``, ``shape``, ``datatype``, ``data``). ``outputs`` is an
        optional list of ``{"name": ...}`` selectors. Empty input lists
        raise ``ValidationError`` -- Triton would reject the request and
        the typed error is more useful than the raw 400.
        """
        if not isinstance(inputs, list) or not inputs:
            raise ValidationError(
                "triton infer requires at least one input tensor",
            )
        body: dict[str, Any] = {"inputs": inputs}
        if outputs:
            body["outputs"] = outputs
        return self._request(
            "POST", f"{model_path}/infer", body, model_hint=model_hint,
        )


# ─── Helpers ───────────────────────────────────────────────────────────────


def _map_http_error(
    status: int,
    body_text: str,
    model_hint: str | None,
    timeout: float,
) -> None:
    """Map a Triton HTTP error to the right typed MAI adapter error.

    Always raises. Caller's ``raise BackendUnavailableError`` after a
    call here is unreachable, but mypy needs it.
    """
    detail = _extract_error_detail(body_text)
    lower = detail.lower()

    if status == 404:
        raise ModelNotFoundError(model=model_hint or "unknown")
    if status in (408, 504):
        raise AdapterTimeoutError(timeout_ms=int(timeout * 1000))
    if status == 429:
        raise RateLimitedError()
    if (
        status == 413
        or "context" in lower
        or "too long" in lower
        or "exceed" in lower
    ):
        raise ContextExceededError(max_context=0)
    if _OOM_PATTERN.search(detail) or ("cuda" in lower and "memory" in lower):
        raise OutOfMemoryError()
    if status == 502 or "broken pipe" in lower or "reset by peer" in lower:
        raise BackendCrashedError(detail=detail or f"HTTP {status}")
    if status >= 500:
        raise BackendUnavailableError(detail=detail or f"HTTP {status}")
    raise BackendUnavailableError(detail=detail or f"HTTP {status}")


def _extract_error_detail(body_text: str) -> str:
    """Pull a useful detail string out of a Triton error body."""
    if not body_text:
        return ""
    try:
        parsed = json.loads(body_text)
    except json.JSONDecodeError:
        return body_text[:200]
    if isinstance(parsed, dict):
        for key in ("error", "message", "detail"):
            value = parsed.get(key)
            if isinstance(value, str) and value:
                return value
    return body_text[:200]
