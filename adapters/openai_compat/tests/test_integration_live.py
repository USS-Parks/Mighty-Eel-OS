"""Live-backend integration tests for the OpenAI-compatible adapter.

These tests hit a REAL local OpenAI-compatible server (LM Studio,
LocalAI, FastChat, an internal gateway, etc.). They SKIP cleanly when
the backend is unavailable, so ``pytest adapters/openai_compat/`` on a
machine without one still runs the mocked unit tests in
``test_adapter.py`` without error.

Opt-in:

.. code:: powershell

    # Point at a running OpenAI-compatible server, then:
    $env:OPENAI_COMPAT_HOST = "http://127.0.0.1:1234"
    pytest -m live_backend `
        adapters/openai_compat/tests/test_integration_live.py -v

Environment variables honoured by the availability fixture (see
``adapters/openai_compat/tests/conftest.py``):

  * ``OPENAI_COMPAT_HOST``  — required base URL.
  * ``OPENAI_COMPAT_MODEL`` — optional chat-capable model id.
  * ``OPENAI_COMPAT_EMBEDDING_MODEL`` — optional embedding model id;
    when unset, the embedding test asserts the negative-path contract
    via ``supports_embeddings=False``.

DOUGHERTY J-23. Closes the live-test gap for the new adapter per
``docs/ADAPTER-COMPLETION-MATRIX.md`` and
``docs/ADAPTER-TEST-HARNESS-LOCK.md`` §Live Backend Test Minimums.
"""

from __future__ import annotations

import os
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
from adapters.openai_compat.adapter import OpenAICompatAdapter

pytestmark = pytest.mark.live_backend


# ─── Helpers ────────────────────────────────────────────────────────────


def _host_to_parts(host: str) -> tuple[str, str, int]:
    """Split ``http://host:port`` into ``(scheme, host, port)``."""
    if "://" in host:
        scheme, rest = host.split("://", 1)
    else:
        scheme, rest = "http", host
    rest = rest.rstrip("/")
    if ":" in rest:
        h, p = rest.split(":", 1)
        return scheme, h, int(p)
    return scheme, rest, 80 if scheme == "http" else 443


async def _adapter_for(
    target: dict[str, Any],
    *,
    supports_embeddings: bool = False,
    embedding_model: str = "",
) -> OpenAICompatAdapter:
    """Initialize a live adapter pointed at the discovered backend."""
    scheme, host, port = _host_to_parts(target["host"])
    cfg = {
        "scheme": scheme,
        "host": host,
        "port": port,
        "default_model": target.get("model", ""),
        "timeout_ms": 30000,
        "stream_timeout_ms": 60000,
        "supports_streaming": True,
        "supports_embeddings": supports_embeddings,
        "embedding_model": embedding_model,
    }
    adapter = OpenAICompatAdapter()
    await adapter.initialize(cfg, hil_handle=None)
    return adapter


# ─── Skip-when-unavailable guard ───────────────────────────────────────


@pytest.fixture(autouse=True)
def require_live_openai_compat(
    openai_compat_available: dict[str, Any] | None,
) -> dict[str, Any]:
    """Module-level skip helper. Every test in this file uses it."""
    if openai_compat_available is None:
        pytest.skip(
            "OPENAI_COMPAT_HOST not set or backend unreachable — "
            "set OPENAI_COMPAT_HOST=http://127.0.0.1:1234 (or your "
            "local server URL) to enable live tests.",
        )
    return openai_compat_available


# ─── Tests ─────────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """The adapter initializes, reports honest capabilities, and the
    readiness probe returned at least one model id."""
    adapter = await _adapter_for(require_live_openai_compat)
    try:
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        # Backend exposed at least one model via /v1/models.
        assert len(adapter._known_models) >= 1
        assert adapter._initialized is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_non_streaming(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """A short ``Say OK.`` prompt returns a populated GenerationResult."""
    adapter = await _adapter_for(require_live_openai_compat)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=32,
        )
        result = await adapter.generate("Say OK.", params, stream=False)
        assert isinstance(result, GenerationResult)
        assert len(result.text.strip()) > 0
        assert result.tokens_generated >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_stream_yields_tokens(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """SSE streaming yields at least one non-empty Token and terminates
    cleanly with ``is_end_of_text=True``."""
    adapter = await _adapter_for(require_live_openai_compat)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=32,
        )
        stream = await adapter.generate(
            "Count: 1, 2, 3.",
            params,
            stream=True,
        )
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)
        assert tokens, "streaming yielded zero tokens"
        non_empty = [t for t in tokens if t.text]
        assert non_empty, "streaming yielded only empty tokens"
        indices = [t.index for t in tokens]
        assert indices == sorted(indices)
        assert tokens[-1].is_end_of_text is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_against_real_server(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """``health_check()`` returns HEALTHY with a non-negative uptime
    after a single generation."""
    adapter = await _adapter_for(require_live_openai_compat)
    try:
        params = GenerationParams(temperature=0.0, max_tokens=8)
        await adapter.generate("Say ok.", params, stream=False)
        status: HealthStatus = await adapter.health_check()
        assert status.kind is HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0
        assert status.requests_served >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_embeddings_unsupported_or_works(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """When ``OPENAI_COMPAT_EMBEDDING_MODEL`` is unset the adapter
    reports ``supports_embedding=False`` and ``embed()`` raises
    ``UnsupportedOperationError``. When the env var is supplied the
    adapter exercises ``/v1/embeddings`` against the live backend and
    returns vectors whose dimensions match across inputs."""
    embedding_model = os.environ.get("OPENAI_COMPAT_EMBEDDING_MODEL", "")
    adapter = await _adapter_for(
        require_live_openai_compat,
        supports_embeddings=bool(embedding_model),
        embedding_model=embedding_model,
    )
    try:
        caps = adapter.capabilities()
        if not caps.supports_embedding:
            with pytest.raises(UnsupportedOperationError):
                await adapter.embed(["hello"])
            assert caps.supports_embedding is False
            return
        vectors = await adapter.embed(["hello", "different text"])
        assert len(vectors) == 2
        assert len(vectors[0].vector) == len(vectors[1].vector)
        assert vectors[0].vector != vectors[1].vector
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_shutdown_idempotent(
    require_live_openai_compat: dict[str, Any],
) -> None:
    """Calling ``shutdown`` twice is a no-op the second time and leaves
    the adapter in a clean uninitialized state."""
    adapter = await _adapter_for(require_live_openai_compat)
    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
