"""OpenAI-compatible adapter orchestration helpers."""

from __future__ import annotations

from collections.abc import Iterator

from adapters.base import Token
from adapters.openai_compat.client import OpenAICompatStreamChunk
from adapters.openai_compat.config import OpenAICompatConfig
from adapters.openai_compat.responses import first_or_empty


def resolve_models(
    config: OpenAICompatConfig, known_models: list[str],
) -> tuple[str, str, str]:
    fallback = config.default_model or first_or_empty(known_models)
    return (
        config.chat_model or fallback,
        config.completion_model or fallback,
        config.embedding_model or fallback,
    )


def chat_messages(prompt: str) -> list[dict[str, str]]:
    return [{"role": "user", "content": prompt}]


def tokens_from_chunks(chunks: Iterator[OpenAICompatStreamChunk]) -> Iterator[Token]:
    token_index = 0
    saw_any = False
    for chunk in chunks:
        saw_any = True
        if chunk.content:
            yield Token(
                text=chunk.content,
                index=token_index,
                is_end_of_text=chunk.stop,
            )
            token_index += 1
        elif chunk.stop:
            yield Token(text="", index=token_index, is_end_of_text=True)
            token_index += 1
    if not saw_any:
        yield Token(text="", index=0, is_end_of_text=True)
