"""HTTP response records and parsing helpers for OpenAI-compatible clients."""

from __future__ import annotations

import json
import urllib.error
from dataclasses import dataclass
from typing import Any


@dataclass
class OpenAICompatResponse:
    """Parsed unary response from an OpenAI-compatible server."""

    status_code: int
    body: dict[str, Any]
    elapsed_ms: float


@dataclass
class OpenAICompatStreamChunk:
    """One SSE event from /v1/chat/completions with stream=true."""

    content: str
    finish_reason: str | None
    stop: bool = False


def read_http_error_body(error: urllib.error.HTTPError) -> str:
    try:
        if error.fp is not None:
            return error.fp.read().decode("utf-8", errors="replace")
    except (OSError, AttributeError):
        return ""
    return ""


def stream_chunk_from_payload(payload: str) -> OpenAICompatStreamChunk | None:
    try:
        event = json.loads(payload)
    except json.JSONDecodeError:
        return None
    choices = event.get("choices") or []
    if not choices:
        return None
    choice = choices[0]
    delta = choice.get("delta") or {}
    content = delta.get("content") or ""
    finish_reason = choice.get("finish_reason")
    return OpenAICompatStreamChunk(
        content=content,
        finish_reason=finish_reason,
        stop=finish_reason is not None,
    )


def error_detail(body_text: str) -> str:
    try:
        err_body = json.loads(body_text) if body_text else {}
    except (json.JSONDecodeError, TypeError):
        return body_text[:200]
    if not isinstance(err_body, dict):
        return ""
    err = err_body.get("error")
    if isinstance(err, dict):
        return str(err.get("message") or err.get("type") or "")
    if isinstance(err, str):
        return err
    return str(err_body.get("message") or "")


def is_context_error(detail_l: str) -> bool:
    return (
        "context" in detail_l
        and ("exceed" in detail_l or "too long" in detail_l or "length" in detail_l)
    )


def is_oom_error(detail_l: str) -> bool:
    return "out of memory" in detail_l or "oom" in detail_l


def extract_model(detail: str) -> str | None:
    """Pull a model id out of a Model 'foo' not found style message."""
    if not detail:
        return None
    for quote in ("'", '"'):
        if quote in detail:
            try:
                left = detail.index(quote) + 1
                right = detail.index(quote, left)
                return detail[left:right]
            except ValueError:
                continue
    return None
