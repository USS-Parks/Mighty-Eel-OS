"""Config validation for OpenAI-compatible adapters."""

from __future__ import annotations

from adapters.base import ValidationError
from adapters.openai_compat.config import OpenAICompatConfig


def validate_config(cfg: OpenAICompatConfig) -> None:
    if cfg.scheme not in ("http", "https"):
        raise ValidationError(f"invalid scheme: {cfg.scheme!r}")
    if not cfg.host:
        raise ValidationError("host must be set")
    if not (0 < cfg.port < 65536):
        raise ValidationError(f"port out of range: {cfg.port}")
    if cfg.prefer_endpoint not in ("chat", "completion"):
        raise ValidationError(
            f"prefer_endpoint must be 'chat' or 'completion', got "
            f"{cfg.prefer_endpoint!r}",
        )
    if cfg.timeout_ms <= 0 or cfg.stream_timeout_ms <= 0:
        raise ValidationError("timeouts must be positive")
    if cfg.max_retries < 0:
        raise ValidationError("max_retries must be >= 0")
