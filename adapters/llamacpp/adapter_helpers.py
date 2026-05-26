"""llama.cpp adapter response parsing and streaming helpers."""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from typing import Any

from adapters.base import FinishReason, GenerationParams, GenerationResult, Token
from adapters.llamacpp.client import LlamaCppClient


def generation_result_from_body(body: dict[str, Any]) -> GenerationResult:
    choices = body.get("choices", [])
    if not choices:
        return GenerationResult(text="", tokens_generated=0)
    choice = choices[0]
    text = choice.get("message", {}).get("content", "")
    finish = choice.get("finish_reason", "stop")
    reason = FinishReason.MAX_TOKENS if finish == "length" else FinishReason.STOP
    tokens_out = body.get("usage", {}).get("completion_tokens", len(text) // 4)
    return GenerationResult(text=text, tokens_generated=tokens_out, finish_reason=reason)


def grammar_for_params(
    default_grammar: str | None, params: GenerationParams,
) -> str | None:
    if not params.structured_schema:
        return default_grammar
    return json.dumps(params.structured_schema) if not default_grammar else default_grammar


async def stream_tokens(
    client: LlamaCppClient,
    prompt: str,
    params: GenerationParams,
    grammar: str | None,
) -> AsyncIterator[Token]:
    messages = [{"role": "user", "content": prompt}]
    chunks = await asyncio.to_thread(
        client.chat_completions,
        messages=messages,
        temperature=params.temperature,
        top_p=params.top_p,
        max_tokens=params.max_tokens,
        stop=params.stop_sequences or None,
        stream=True,
        grammar=grammar,
    )

    token_index = 0
    for chunk in chunks:
        token = token_from_stream_chunk(chunk, token_index)
        if token is None:
            continue
        yield token
        if token.text:
            token_index += 1


def token_from_stream_chunk(chunk: Any, index: int) -> Token | None:
    if chunk.content:
        return Token(text=chunk.content, index=index, is_end_of_text=chunk.stop)
    if chunk.stop:
        return Token(text="", index=index, is_end_of_text=True)
    return None


async def counted_stream(
    owner: Any, stream: AsyncIterator[Token],
) -> AsyncIterator[Token]:
    async for token in stream:
        yield token
    owner._requests_served += 1


async def batch_results(
    client: LlamaCppClient,
    prompts: list[str],
    params: GenerationParams,
) -> list[GenerationResult]:
    results: list[GenerationResult] = []
    for prompt in prompts:
        body = await chat_completion_body(client, prompt, params)
        results.append(generation_result_from_body(body))
    return results


async def chat_completion_body(
    client: LlamaCppClient,
    prompt: str,
    params: GenerationParams,
) -> dict[str, Any]:
    messages = [{"role": "user", "content": prompt}]
    resp = await asyncio.to_thread(
        client.chat_completions,
        messages=messages,
        temperature=params.temperature,
        top_p=params.top_p,
        max_tokens=params.max_tokens,
        stop=params.stop_sequences or None,
        stream=False,
    )
    return resp.body
