"""TensorRT adapter validation and response helpers."""

from __future__ import annotations

import time
from typing import Any

from adapters.base import FinishReason, GenerationParams, ValidationError
from adapters.tensorrt.config import TensorRtConfig

STREAM_SENTINEL: Any = object()


def validate_config(config: TensorRtConfig) -> None:
    """Reject obviously-invalid TensorRT configs with a typed ValidationError."""
    if not config.host:
        raise ValidationError("host must be set")
    if config.port <= 0 or config.port > 65535:
        raise ValidationError(f"port out of range: {config.port}")
    if config.timeout_ms <= 0:
        raise ValidationError(f"timeout_ms must be positive: {config.timeout_ms}")
    if config.stream_timeout_ms <= 0:
        raise ValidationError(
            f"stream_timeout_ms must be positive: {config.stream_timeout_ms}",
        )
    if not config.default_model:
        raise ValidationError("default_model must be set (Triton model name)")


def result_from_body(
    body: dict[str, Any], params: GenerationParams,
) -> tuple[str, int, FinishReason]:
    """Extract (text, tokens_generated, finish_reason) from a Triton body."""
    text = ""
    if "text_output" in body:
        text = body.get("text_output") or ""
    elif "choices" in body:
        choices = body.get("choices") or []
        if choices and isinstance(choices[0], dict):
            text = choices[0].get("text") or choices[0].get("message", {}).get(
                "content", "",
            ) or ""
    tokens_out_raw = body.get("output_tokens") or body.get("generated_tokens")
    if isinstance(tokens_out_raw, int) and tokens_out_raw >= 0:
        tokens_out = tokens_out_raw
    else:
        tokens_out = max(1, len(text) // 4) if text else 0
    finish_raw = body.get("finish_reason") or body.get("stop_reason") or "stop"
    finish = map_finish_reason(finish_raw, tokens_out, params)
    return text, tokens_out, finish


def map_finish_reason(
    raw: str, tokens_out: int, params: GenerationParams,
) -> FinishReason:
    """Map Triton's finish-reason string into the MAI enum."""
    if raw in ("length", "max_tokens"):
        return FinishReason.MAX_TOKENS
    if raw == "stop_sequence" or (params.stop_sequences and raw == "stop"):
        return FinishReason.STOP_SEQUENCE if raw == "stop_sequence" else FinishReason.STOP
    if tokens_out >= params.max_tokens:
        return FinishReason.MAX_TOKENS
    return FinishReason.STOP


def next_or_sentinel(iterator: Any) -> Any:
    """next(iterator) that returns a sentinel on StopIteration."""
    try:
        return next(iterator)
    except StopIteration:
        return STREAM_SENTINEL


def now_ms() -> int:
    return int(time.time() * 1000)
