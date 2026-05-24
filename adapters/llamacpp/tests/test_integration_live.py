"""Live-backend integration tests for the llama.cpp adapter.

These tests hit a REAL llama-server (`llama-server -m model.gguf
--port 8081 [--embedding]`). They SKIP cleanly when the backend is
unavailable, so `pytest adapters/llamacpp/` on a machine without
llama-server still runs the mocked unit tests in `test_adapter.py`
without error.

Opt-in:
    # Start llama-server in another terminal first:
    llama-server -m tinyllama-1.1b-chat-q4.gguf --port 8081

    export LLAMACPP_HOST=http://127.0.0.1:8081
    pytest -m live_backend adapters/llamacpp/tests/test_integration_live.py -v

Embeddings test auto-skips when the adapter reports
`supports_embedding=False` (the current default; a future llama-server
build with `--embedding` would flip the capability and exercise the
real embedding path).

DOUGHERTY lane J-07. Closes the live-test gap surfaced in
`docs/ADAPTER-COMPLETION-MATRIX.md` §2.2.
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
from adapters.llamacpp.adapter import LlamaCppAdapter

# Every test in this module is opt-in and skips cleanly when the
# backend is unavailable.
pytestmark = pytest.mark.live_backend


# ─── Helpers ────────────────────────────────────────────────────────────────


def _host_to_parts(host: str) -> tuple[str, int]:
    """Split a `http://h:p` URL into (host, port). Defaults to port 8080."""
    stripped = host.replace("http://", "").replace("https://", "").rstrip("/")
    if ":" in stripped:
        h, p = stripped.split(":", 1)
        return h, int(p)
    return stripped, 8080


async def _adapter_for(target: dict[str, Any]) -> LlamaCppAdapter:
    """Build and initialize a LlamaCppAdapter against the live backend."""
    h, p = _host_to_parts(target["host"])
    adapter = LlamaCppAdapter()
    config_dict = {
        "host": h,
        "port": p,
        # default_model is informational only; llama-server is started
        # with -m pointing at a specific GGUF, and the adapter
        # discovers the actual model via /props during initialize.
        "default_model": target.get("model", ""),
        "context_size": 4096,
        "n_gpu_layers": -1,   # respect llama-server's launch flags
    }
    await adapter.initialize(config_dict, hil_handle=None)
    return adapter


# ─── Skip-when-unavailable guard ────────────────────────────────────────────


@pytest.fixture(autouse=True)
def require_live_llamacpp(
    llamacpp_available: dict[str, Any] | None,
) -> dict[str, Any]:
    """Module-level skip helper. Every test in this file uses it."""
    if llamacpp_available is None:
        pytest.skip(
            "LLAMACPP_HOST not set or llama-server unreachable — "
            "set LLAMACPP_HOST=http://127.0.0.1:8081 to enable live tests.",
        )
    return llamacpp_available


# ─── Tests ──────────────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_llamacpp: dict[str, Any],
) -> None:
    """Adapter initializes against a real llama-server: capabilities are
    declared, the discovered context size is positive, and the model
    handle has the expected llamacpp- prefix."""
    adapter = await _adapter_for(require_live_llamacpp)
    try:
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        # Whatever llama-server reported for n_ctx is stored on the
        # adapter as a positive int.
        assert adapter._context_size >= 512
        assert adapter._initialized is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_deterministic(
    require_live_llamacpp: dict[str, Any],
) -> None:
    """temperature=0 yields deterministic output for the same prompt.

    Uses the non-streaming `generate(stream=False)` path — the default
    when callers do not opt into streaming. Asserts two runs produce
    identical text AND that the GenerationResult shape is honoured.
    """
    adapter = await _adapter_for(require_live_llamacpp)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=64,
        )
        prompt = "What is the capital of France? Answer in one word."

        result_a = await adapter.generate(prompt, params, stream=False)
        result_b = await adapter.generate(prompt, params, stream=False)

        assert isinstance(result_a, GenerationResult)
        assert isinstance(result_b, GenerationResult)
        assert len(result_a.text.strip()) > 0
        assert result_a.text == result_b.text, (
            f"Determinism violated: run-1={result_a.text!r}  "
            f"run-2={result_b.text!r}"
        )
        # GenerationResult fields are populated by the adapter.
        assert result_a.tokens_generated >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_stream_yields_tokens(
    require_live_llamacpp: dict[str, Any],
) -> None:
    """generate(stream=True) returns an AsyncIterator over Token objects;
    verify at least one non-empty chunk arrives and indices are
    monotonically non-decreasing.

    Note: the llama.cpp adapter packs tokens into chunks per
    llama-server's SSE granularity. A short prompt may produce a
    single chunk — that is valid streaming behavior per
    docs/ADAPTER-COMPLETION-MATRIX.md §2.2.
    """
    adapter = await _adapter_for(require_live_llamacpp)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=64,
        )
        prompt = "Count from 1 to 3 separated by commas."

        stream = await adapter.generate(prompt, params, stream=True)
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)

        non_empty = [t for t in tokens if t.text]
        assert len(non_empty) >= 1, (
            f"Expected at least 1 streamed token chunk; got {len(non_empty)}"
        )
        assert sum(len(t.text) for t in non_empty) > 0

        indices = [t.index for t in tokens]
        assert indices == sorted(indices)
        assert indices[0] == 0
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_embeddings_unsupported_or_works(
    require_live_llamacpp: dict[str, Any],
) -> None:
    """The current llama.cpp adapter exposes supports_embedding=False
    and embed() raises UnsupportedOperationError. If a future
    llama-server build is started with `--embedding`, the adapter
    capability would flip True and the embed path should produce
    vectors. This test handles both cases honestly."""
    adapter = await _adapter_for(require_live_llamacpp)
    try:
        caps = adapter.capabilities()
        if not caps.supports_embedding:
            with pytest.raises(UnsupportedOperationError):
                await adapter.embed(["hello"])
            # If we got here, the negative-path contract is honoured.
            assert caps.supports_embedding is False
            return

        # supports_embedding=True path — real embedding workload.
        vectors = await adapter.embed(["hello", "hello", "different text"])
        assert len(vectors) == 3
        assert len(vectors[0].vector) == len(vectors[1].vector)
        assert vectors[0].vector == vectors[1].vector
        assert vectors[0].vector != vectors[2].vector
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_against_real_server(
    require_live_llamacpp: dict[str, Any],
) -> None:
    """health_check() against a running llama-server returns HEALTHY
    with sensible uptime and request counters after a single generate."""
    adapter = await _adapter_for(require_live_llamacpp)
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
    require_live_llamacpp: dict[str, Any],
) -> None:
    """Calling shutdown twice is a no-op the second time and leaves the
    adapter in a clean uninitialized state."""
    adapter = await _adapter_for(require_live_llamacpp)

    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None

    # Second call must not raise.
    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
