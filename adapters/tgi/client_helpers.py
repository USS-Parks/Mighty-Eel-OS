"""TGI client HTTP error mapping helpers."""

from __future__ import annotations

import json

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    ContextExceededError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    ValidationError,
)


def raise_for_http_error(status: int, body_text: str, timeout: float) -> None:
    if status == 429:
        raise RateLimitedError()
    if status in (408, 504):
        raise AdapterTimeoutError(timeout_ms=int(timeout * 1000))

    detail = ""
    error_type = ""
    try:
        err_body = json.loads(body_text) if body_text else {}
        if isinstance(err_body, dict):
            detail = str(err_body.get("error", ""))
            error_type = str(err_body.get("error_type", ""))
    except json.JSONDecodeError:
        detail = body_text[:200]

    low_detail = detail.lower()
    low_type = error_type.lower()
    if low_type == "overloaded" or "overloaded" in low_detail:
        raise RateLimitedError()
    if "memory" in low_detail or "oom" in low_detail or "cuda out of memory" in low_detail:
        raise OutOfMemoryError()
    if low_type == "validation" and (
        "too long" in low_detail or "max_input" in low_detail or "context" in low_detail
    ):
        raise ContextExceededError(max_context=0)
    if low_type == "validation" or status == 422:
        raise ValidationError(detail or "TGI validation error")
    if status == 404:
        raise ModelNotFoundError(detail or "TGI model not found")
    if status >= 500:
        raise BackendUnavailableError(detail or f"TGI server error {status}")
