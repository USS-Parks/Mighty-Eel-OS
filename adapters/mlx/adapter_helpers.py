"""MLX adapter lifecycle, stream, and capability helpers."""

from __future__ import annotations

import asyncio
import time
from collections.abc import AsyncIterator
from typing import Any

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    GenerationParams,
    HealthStatus,
    ModelNotFoundError,
    Token,
)
from adapters.mlx.client import MLXClient, MLXLoadError, is_apple_silicon


def raise_load_error(error: MLXLoadError, model_path: str) -> None:
    msg = str(error).lower()
    if "not found" in msg or "no such" in msg:
        raise ModelNotFoundError(model_path) from error
    raise BackendUnavailableError(detail=str(error)) from error


async def stream_from_client(
    client: MLXClient,
    prompt: str,
    params: GenerationParams,
    stream_timeout_ms: int,
) -> AsyncIterator[Token]:
    deadline = time.monotonic() + (stream_timeout_ms / 1000.0)
    chunks_iter = await asyncio.to_thread(
        client.stream_generate,
        prompt,
        max_tokens=params.max_tokens,
        temperature=params.temperature,
        top_p=params.top_p,
    )
    async for token in stream_tokens(chunks_iter, deadline, stream_timeout_ms):
        yield token


async def stream_tokens(
    chunks_iter: Any,
    deadline: float,
    timeout_ms: int,
) -> AsyncIterator[Token]:
    index = 0
    for chunk in chunks_iter:
        if time.monotonic() > deadline:
            raise AdapterTimeoutError(timeout_ms)
        if not chunk:
            continue
        yield Token(text=chunk, index=index, is_end_of_text=False)
        index += 1
    yield Token(text="", index=index, is_end_of_text=True)


async def counted_stream(
    owner: Any, stream: AsyncIterator[Token],
) -> AsyncIterator[Token]:
    try:
        async for token in stream:
            yield token
    except MLXLoadError as e:
        raise BackendUnavailableError(detail=str(e)) from e
    owner._requests_served += 1


def lost_handle_status(start_time_ms: int) -> HealthStatus:
    return HealthStatus.degraded(
        reason="MLX client lost model handle",
        uptime_ms=int(time.time() * 1000) - start_time_ms,
    )


def capability_extra() -> dict[str, Any]:
    return {
        "in_process": True,
        "apple_silicon_only": True,
        "platform_ok": is_apple_silicon(),
    }


async def run_generate(
    client: MLXClient,
    prompt: str,
    params: GenerationParams,
    timeout_ms: int,
) -> tuple[str, int, bool]:
    return await asyncio.wait_for(
        asyncio.to_thread(
            client.generate,
            prompt,
            max_tokens=params.max_tokens,
            temperature=params.temperature,
            top_p=params.top_p,
        ),
        timeout=timeout_ms / 1000.0,
    )
