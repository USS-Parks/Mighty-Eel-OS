"""ONNX Runtime adapter result conversion helpers."""

from __future__ import annotations

import time
from collections.abc import Iterator

from adapters.base import Embedding, FinishReason, GenerationResult, HealthStatus, Token
from adapters.onnxruntime.client import OnnxStreamChunk


def generation_result(text: str, count: int, max_tokens: int) -> GenerationResult:
    finish = FinishReason.MAX_TOKENS if count >= max_tokens else FinishReason.STOP
    return GenerationResult(
        text=text,
        tokens_generated=count,
        finish_reason=finish,
    )


def tokens_from_chunks(chunks: list[OnnxStreamChunk]) -> Iterator[Token]:
    token_index = 0
    for chunk in chunks:
        if chunk.is_final:
            yield Token(text="", index=token_index, is_end_of_text=True)
            continue
        yield Token(text=chunk.text, index=token_index, is_end_of_text=False)
        token_index += 1


def embeddings_from_vectors(
    vectors: list[list[float]], texts: list[str],
) -> list[Embedding]:
    return [
        Embedding(vector=v, input_tokens=max(1, len(t.split())))
        for v, t in zip(vectors, texts, strict=False)
    ]


def readiness_status(
    *,
    start_time_ms: int,
    requests_served: int,
    client_ready: bool,
    supports_generation: bool,
    supports_embedding: bool,
) -> HealthStatus:
    uptime = int(time.time() * 1000) - start_time_ms
    if not client_ready:
        return HealthStatus.degraded(
            reason="ONNX Runtime client not ready", uptime_ms=uptime,
        )
    if not (supports_generation or supports_embedding):
        return HealthStatus.degraded(
            reason="loaded model exposes neither generation nor embedding",
            uptime_ms=uptime,
        )
    return HealthStatus.healthy(uptime_ms=uptime, requests_served=requests_served)
