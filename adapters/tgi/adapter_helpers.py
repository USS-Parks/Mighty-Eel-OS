"""TGI adapter response parsing and iterator helpers."""

from __future__ import annotations

from typing import Any

from adapters.base import FinishReason, GenerationResult, ValidationError
from adapters.tgi.client import TgiResponse
from adapters.tgi.config import TgiConfig

STREAM_SENTINEL: Any = object()


def body_dict(resp: TgiResponse | dict[str, Any]) -> dict[str, Any]:
    if isinstance(resp, dict):
        return resp
    body = getattr(resp, "body", None)
    if isinstance(body, dict):
        return body
    return {}


def result_from_body(body: dict[str, Any]) -> GenerationResult:
    generated = body.get("generated_text", "")
    details = body.get("details") or {}
    tokens_out = details.get("generated_tokens", len(generated) // 4)
    finish = details.get("finish_reason", "stop_sequence")
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    return GenerationResult(
        text=generated,
        tokens_generated=tokens_out,
        finish_reason=reason,
    )


def next_or_sentinel(iterator: Any) -> Any:
    try:
        return next(iterator)
    except StopIteration:
        return STREAM_SENTINEL


def validate_config(config: TgiConfig) -> None:
    if not isinstance(config.host, str) or not config.host:
        raise ValidationError("TGI host must be a non-empty string")
    if not isinstance(config.port, int) or config.port <= 0:
        raise ValidationError("TGI port must be a positive integer")
    if config.timeout_ms <= 0 or config.stream_timeout_ms <= 0:
        raise ValidationError("TGI timeouts must be positive integers")
