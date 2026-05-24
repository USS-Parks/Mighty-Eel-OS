"""Live-backend integration tests for the TGI adapter.

These tests hit a REAL HF Text Generation Inference server. They SKIP
cleanly when the backend is unavailable, so ``pytest adapters/tgi/`` on
a machine without TGI still runs the mocked unit tests in
``test_adapter.py`` and the local-fake tests in
``test_integration_mock.py`` without error.

Opt-in:
    # Start TGI in another terminal first, for example:
    #   docker run --gpus all --shm-size 1g -p 8080:80 -v $PWD/data:/data \
    #     ghcr.io/huggingface/text-generation-inference:2.0 \
    #     --model-id mistralai/Mistral-7B-Instruct-v0.2

    export TGI_HOST=http://127.0.0.1:8080
    pytest -m live_backend adapters/tgi/tests/test_integration_live.py -v

The adapter never auto-pulls a model (air-gap policy from CLAUDE.md);
the operator is expected to launch TGI against a model already on disk.

DOUGHERTY J-19. Closes the live-test gap recorded in
``docs/ADAPTER-COMPLETION-MATRIX.md`` for TGI and required by
``docs/ADAPTER-TEST-HARNESS-LOCK.md`` (live-backend test minimums).
"""
from __future__ import annotations

from typing import Any

import pytest

from adapters.base import (
    AdapterCapabilities,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    HealthStatusKind,
    Token,
    UnsupportedOperationError,
)
from adapters.tgi.adapter import TgiAdapter

# Every test in this module is opt-in and skips cleanly when the
# backend is unavailable.
pytestmark = pytest.mark.live_backend


# ----- helpers ----------------------------------------------------------


def _host_to_parts(host: str) -> tuple[str, int]:
    """Split a ``http://h:p`` URL into ``(host, port)``. Default port 8080."""
    stripped = host.replace("http://", "").replace("https://", "").rstrip("/")
    if ":" in stripped:
        h, p = stripped.split(":", 1)
        return h, int(p)
    return stripped, 8080


async def _adapter_for(target: dict[str, Any]) -> TgiAdapter:
    """Build and initialize a TgiAdapter against the live backend."""
    h, p = _host_to_parts(target["host"])
    adapter = TgiAdapter()
    config_dict = {
        "host": h,
        "port": p,
        # TGI serves a single model per process; the adapter discovers
        # the actual model id via /info during initialize. The default
        # is informational only.
        "default_model": target.get("model_id", ""),
        "timeout_ms": 60_000,
        "stream_timeout_ms": 180_000,
    }
    await adapter.initialize(config_dict, hil_handle=None)
    return adapter


# ----- skip guard -------------------------------------------------------


@pytest.fixture(autouse=True)
def require_live_tgi(tgi_available: dict[str, Any] | None) -> dict[str, Any]:
    """Module-level skip helper. Every test in this file uses it."""
    if tgi_available is None:
        pytest.skip(
            "TGI_HOST not set or TGI server unreachable - "
            "set TGI_HOST=http://127.0.0.1:8080 to enable live tests.",
        )
    return tgi_available


# ----- tests ------------------------------------------------------------


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_tgi: dict[str, Any],
) -> None:
    """Adapter initializes against a real TGI server, capabilities are
    declared truthfully for the live backend, and the model id discovered
    via /info is propagated onto the adapter."""
    adapter = await _adapter_for(require_live_tgi)
    try:
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        # TGI never exposes embeddings through this adapter.
        assert caps.supports_embedding is False
        assert adapter._initialized is True
        assert adapter._model_id, "TGI /info returned no model_id"
        assert adapter._max_total_tokens >= 512
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_non_streaming_returns_result(
    require_live_tgi: dict[str, Any],
) -> None:
    """``generate(stream=False)`` returns a populated ``GenerationResult``
    against a real TGI server with a tiny token budget."""
    adapter = await _adapter_for(require_live_tgi)
    try:
        params = GenerationParams(temperature=0.0, top_p=1.0, max_tokens=16)
        result = await adapter.generate("Say OK.", params, stream=False)
        assert isinstance(result, GenerationResult)
        assert len(result.text) > 0
        assert result.tokens_generated >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_streaming_yields_tokens(
    require_live_tgi: dict[str, Any],
) -> None:
    """``generate(stream=True)`` returns an ``AsyncIterator`` over
    ``Token`` objects with monotonically non-decreasing indices."""
    adapter = await _adapter_for(require_live_tgi)
    try:
        params = GenerationParams(temperature=0.0, top_p=1.0, max_tokens=32)
        stream = await adapter.generate(
            "Count from 1 to 3 separated by commas.", params, stream=True,
        )
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)
        non_empty = [t for t in tokens if t.text]
        assert len(non_empty) >= 1, (
            f"Expected at least 1 streamed token chunk; got {len(non_empty)}"
        )
        indices = [t.index for t in tokens]
        assert indices == sorted(indices)
        assert indices[0] == 0
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_embed_raises_unsupported(
    require_live_tgi: dict[str, Any],
) -> None:
    """TGI never supports embeddings; the live backend must honour the
    capability flag and the negative-path contract."""
    adapter = await _adapter_for(require_live_tgi)
    try:
        caps = adapter.capabilities()
        assert caps.supports_embedding is False
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_against_real_server(
    require_live_tgi: dict[str, Any],
) -> None:
    """``health_check()`` against a running TGI returns HEALTHY with
    sensible uptime and request counters after a single generate."""
    adapter = await _adapter_for(require_live_tgi)
    try:
        params = GenerationParams(temperature=0.0, max_tokens=8)
        result = await adapter.generate("Say ok.", params, stream=False)
        assert isinstance(result, GenerationResult)

        status: HealthStatus = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.uptime_ms is not None
        assert status.uptime_ms >= 0
        assert status.requests_served is not None
        assert status.requests_served >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_shutdown_idempotent(
    require_live_tgi: dict[str, Any],
) -> None:
    """Calling shutdown twice is a no-op the second time and leaves the
    adapter in a clean uninitialized state."""
    adapter = await _adapter_for(require_live_tgi)

    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None

    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
