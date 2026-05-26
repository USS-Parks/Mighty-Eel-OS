"""Response normalization helpers for OpenAI-compatible adapters."""

from __future__ import annotations

from typing import Any

from adapters.base import Embedding, FinishReason, GenerationResult, ValidationError
from adapters.openai_compat.client import OpenAICompatResponse


def extract_model_ids(payload: dict[str, Any]) -> list[str]:
    """Pull model ids out of common GET /v1/models response shapes."""
    if not isinstance(payload, dict):
        return []
    data = payload.get("data")
    if isinstance(data, list):
        ids = [str(m.get("id")) for m in data if isinstance(m, dict) and m.get("id")]
        if ids:
            return ids
    models = payload.get("models")
    if isinstance(models, list):
        return [str(m) for m in models if m]
    return []


def first_or_empty(items: list[str]) -> str:
    return items[0] if items else ""


def result_from_chat(resp: OpenAICompatResponse) -> GenerationResult:
    """Map a unary chat-completion response to a GenerationResult."""
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    choices = body.get("choices") or []
    if not choices:
        return GenerationResult(text="", tokens_generated=0, finish_reason=FinishReason.STOP)
    choice = choices[0]
    message = choice.get("message") or {}
    text = str(message.get("content") or "")
    finish = str(choice.get("finish_reason") or "stop")
    usage = body.get("usage") or {}
    tokens_out = int(usage.get("completion_tokens") or max(len(text) // 4, 1 if text else 0))
    return GenerationResult(
        text=text,
        tokens_generated=tokens_out,
        finish_reason=finish_from_str(finish),
    )


def result_from_completion(resp: OpenAICompatResponse) -> GenerationResult:
    """Map a unary text-completion response to a GenerationResult."""
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    choices = body.get("choices") or []
    if not choices:
        return GenerationResult(text="", tokens_generated=0, finish_reason=FinishReason.STOP)
    choice = choices[0]
    text = str(choice.get("text") or "")
    finish = str(choice.get("finish_reason") or "stop")
    usage = body.get("usage") or {}
    tokens_out = int(usage.get("completion_tokens") or max(len(text) // 4, 1 if text else 0))
    return GenerationResult(
        text=text,
        tokens_generated=tokens_out,
        finish_reason=finish_from_str(finish),
    )


def finish_from_str(value: str) -> FinishReason:
    v = value.lower()
    if v == "length":
        return FinishReason.MAX_TOKENS
    if v == "stop_sequence":
        return FinishReason.STOP_SEQUENCE
    return FinishReason.STOP


def embeddings_from_response(
    resp: OpenAICompatResponse,
    expected: int,
) -> list[Embedding]:
    body = resp.body if isinstance(resp, OpenAICompatResponse) else resp
    data = body.get("data") if isinstance(body, dict) else None
    if not isinstance(data, list):
        raise ValidationError("embeddings response missing 'data' list")
    usage = body.get("usage") if isinstance(body, dict) else None
    total_prompt_tokens = 0
    if isinstance(usage, dict):
        total_prompt_tokens = int(usage.get("prompt_tokens") or 0)
    per_input_tokens = (
        total_prompt_tokens // expected if expected and total_prompt_tokens else 0
    )

    indexed: list[tuple[int, Embedding]] = []
    for fallback_idx, entry in enumerate(data):
        if not isinstance(entry, dict):
            raise ValidationError("embedding entry must be an object")
        raw_vector = entry.get("embedding")
        if not isinstance(raw_vector, list) or not raw_vector:
            raise ValidationError("embedding entry missing non-empty vector")
        vector = [float(x) for x in raw_vector]
        idx = entry.get("index")
        idx_int = int(idx) if isinstance(idx, int) else fallback_idx
        indexed.append((idx_int, Embedding(vector=vector, input_tokens=per_input_tokens)))
    indexed.sort(key=lambda pair: pair[0])
    return [emb for _, emb in indexed]
