"""Ollama adapter model, option, and token helpers."""

from __future__ import annotations

import logging
import time
from collections.abc import Iterator
from typing import Any

from adapters.base import (
    AdapterError,
    FinishReason,
    GenerationParams,
    GenerationResult,
    ModelNotFoundError,
    Token,
)
from adapters.ollama.client import OllamaStreamChunk

logger = logging.getLogger("mai.adapters.ollama")


def stream_tokens(chunks: list[OllamaStreamChunk]) -> Iterator[Token]:
    token_index = 0
    for chunk in chunks:
        if not chunk.content:
            continue
        yield Token(
            text=chunk.content,
            logprob=None,
            index=token_index,
            is_end_of_text=chunk.done,
        )
        token_index += 1
    if not chunks or not chunks[-1].done:
        yield Token(text="", logprob=None, index=token_index, is_end_of_text=True)


def generation_result_from_body(body: dict[str, Any]) -> GenerationResult:
    text = body.get("response", "")
    eval_count = body.get("eval_count", 0)
    finish = FinishReason.MAX_TOKENS if body.get("done_reason") == "length" else FinishReason.STOP
    return GenerationResult(
        text=text,
        tokens_generated=eval_count,
        finish_reason=finish,
    )


def resolve_required_model(model: str, models: list[str]) -> str:
    if model in models:
        return model
    base_name = model.split(":", maxsplit=1)[0]
    matched = next((m for m in models if m.startswith(base_name)), None)
    if matched is None:
        raise ModelNotFoundError(model=model)
    return matched


def not_ready_error() -> AdapterError:
    return AdapterError(
        code="NotReady",
        detail="Adapter not initialized. Call initialize() first.",
    )


def params_to_ollama_options(params: GenerationParams) -> dict[str, Any]:
    options: dict[str, Any] = {
        "temperature": params.temperature,
        "top_p": params.top_p,
        "num_predict": params.max_tokens,
    }
    if params.stop_sequences:
        options["stop"] = params.stop_sequences
    return options


def resolve_default_model(model: str, available_models: list[str]) -> str:
    if not model or model in available_models:
        return model

    base_name = model.split(":", maxsplit=1)[0]
    matched = next((m for m in available_models if m.startswith(base_name)), None)
    if matched is not None:
        logger.info(f"Exact model '{model}' not found, using closest match: '{matched}'")
        return matched

    logger.warning(
        f"Default model '{model}' not available locally. Available: {available_models}",
    )
    return model


def now_ms() -> int:
    return int(time.time() * 1000)
