"""vLLM adapter response parsing helpers."""

from __future__ import annotations

from typing import Any

from adapters.base import Embedding, FinishReason, GenerationParams, GenerationResult, Token


def body_from_response(resp: Any) -> dict[str, Any]:
    if isinstance(resp, dict):
        return resp
    return resp.body if hasattr(resp, "body") else resp


def chat_messages(prompt: str) -> list[dict[str, str]]:
    return [{"role": "user", "content": prompt}]


def chat_kwargs(params: GenerationParams) -> dict[str, Any]:
    if params.structured_schema:
        return {"guided_json": params.structured_schema}
    return {}


def generation_result_from_body(body: dict[str, Any]) -> GenerationResult:
    choices = body.get("choices", [])
    if not choices:
        return GenerationResult(text="", tokens_generated=0)

    choice = choices[0]
    text = choice.get("message", {}).get("content", "")
    finish = choice.get("finish_reason", "stop")
    usage = body.get("usage", {})
    tokens_out = usage.get("completion_tokens", len(text) // 4)
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    return GenerationResult(text=text, tokens_generated=tokens_out, finish_reason=reason)


def token_from_chunk(chunk: Any, index: int) -> Token | None:
    if chunk.content:
        return Token(
            text=chunk.content,
            index=index,
            is_end_of_text=chunk.finish_reason is not None,
        )
    if chunk.finish_reason:
        return Token(text="", index=index, is_end_of_text=True)
    return None


def embeddings_from_body(body: dict[str, Any], texts: list[str]) -> list[Embedding]:
    data = body.get("data", [])
    usage = body.get("usage", {})
    total_tokens = usage.get("total_tokens", sum(len(t) // 4 for t in texts))
    per_text_tokens = total_tokens // max(len(texts), 1)
    return [
        Embedding(vector=item.get("embedding", []), input_tokens=per_text_tokens)
        for item in data
    ]


def resolve_default_model(default_model: str, available_models: list[str]) -> str:
    if not available_models or default_model in available_models:
        return default_model
    for model in available_models:
        if default_model in model or model in default_model:
            return model
    return available_models[0]
