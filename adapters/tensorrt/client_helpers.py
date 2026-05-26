"""Triton response records, error mapping, and SSE parsing helpers."""

from __future__ import annotations

import json
from collections.abc import Iterator
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
)


@dataclass
class TritonResponse:
    """Parsed Triton API response."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class TritonStreamChunk:
    """Single chunk from Triton streaming response."""

    text: str
    finished: bool = False
    cum_log_prob: float | None = None


def map_http_error(
    status: int,
    body_text: str,
    model_hint: str | None,
    timeout: float,
) -> None:
    """Map a Triton HTTP error to the right typed MAI adapter error."""
    detail = extract_error_detail(body_text)
    lower = detail.lower()

    if status == 404:
        raise ModelNotFoundError(model=model_hint or "unknown")
    if status in (408, 504):
        raise AdapterTimeoutError(timeout_ms=int(timeout * 1000))
    if status == 429:
        raise RateLimitedError()
    if status == 413 or "context" in lower or "too long" in lower or "exceed" in lower:
        raise ContextExceededError(max_context=0)
    if "out of memory" in lower or "oom" in lower or ("cuda" in lower and "memory" in lower):
        raise OutOfMemoryError()
    if status == 502 or "broken pipe" in lower or "reset by peer" in lower:
        raise BackendCrashedError(detail=detail or f"HTTP {status}")
    if status >= 500:
        raise BackendUnavailableError(detail=detail or f"HTTP {status}")
    raise BackendUnavailableError(detail=detail or f"HTTP {status}")


def extract_error_detail(body_text: str) -> str:
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


def iter_sse_chunks(resp: Any) -> Iterator[TritonStreamChunk]:
    """Parse Triton's SSE stream into TritonStreamChunk objects."""
    for raw_line in resp:
        line = raw_line.decode("utf-8", errors="replace").strip()
        if not line or not line.startswith("data:"):
            continue
        payload = line[len("data:") :].strip()
        if payload == "[DONE]":
            return
        try:
            data = json.loads(payload)
        except json.JSONDecodeError as exc:
            raise BackendCrashedError(
                detail=f"malformed SSE frame from Triton: {exc}",
            ) from exc
        text, finished = extract_stream_text(data)
        yield TritonStreamChunk(text=text, finished=finished)
        if finished:
            return


def extract_stream_text(data: dict[str, Any]) -> tuple[str, bool]:
    """Normalize the two SSE frame shapes Triton/TensorRT-LLM can emit."""
    if "choices" in data:
        choices = data["choices"]
        if isinstance(choices, list) and choices:
            choice = choices[0]
            delta = choice.get("delta") if isinstance(choice, dict) else None
            if isinstance(delta, dict):
                text = delta.get("content", "")
            else:
                text = choice.get("text", "") if isinstance(choice, dict) else ""
            finished = bool(
                isinstance(choice, dict) and choice.get("finish_reason") is not None,
            )
            return text or "", finished
    if "text_output" in data:
        text = data.get("text_output", "")
        finished = bool(data.get("is_final", False))
        return text or "", finished
    return "", bool(data.get("is_final", False))
