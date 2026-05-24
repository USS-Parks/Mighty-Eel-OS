"""Live-backend integration tests for the ExLlamaV2 adapter.

These tests hit a REAL ExLlamaV2 / TabbyAPI server (OpenAI-compatible
`/v1/chat/completions` + `/v1/models`). They SKIP cleanly when the
backend is unavailable, so `pytest adapters/exllamav2/` on a machine
without TabbyAPI still runs the mocked unit tests in `test_adapter.py`
without error.

Opt-in:
    # Start TabbyAPI (or equivalent ExLlamaV2 HTTP server) first:
    #   python -m tabbyapi  --port 5000  --model <exl2-or-gptq-model>

    export EXLLAMAV2_HOST=http://127.0.0.1:5000
    pytest -m live_backend adapters/exllamav2/tests/test_integration_live.py -v

ADAPTER-SHARED-CONTRACT.md prohibits parallel J-18..J-26 sessions from
editing shared test fixtures, so the EXLLAMAV2_HOST gate lives in this
file rather than in `mai/conftest.py`. The convergence pass owns the
shared-fixture rollup.

Embeddings test asserts `UnsupportedOperationError` because the
adapter's `capabilities().supports_embedding` is False (ExLlamaV2 has
no embedding endpoint). If a future TabbyAPI build adds one and the
adapter capability flips True, this test will fail loudly per
§Capability Truthfulness.

DOUGHERTY lane J-21. Pairs with the J-09 assertion-fill work that
already covers the mocked path.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
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
from adapters.exllamav2.adapter import ExLlamaV2Adapter

# Every test in this module is opt-in and skips cleanly when the
# backend is unavailable.
pytestmark = pytest.mark.live_backend


# ─── Skip-when-unavailable gate (local, per ADAPTER-SHARED-CONTRACT) ────────


def _http_get_json(url: str, timeout_s: float) -> dict[str, Any] | None:
    """Stdlib-only GET that returns parsed JSON or None on any failure."""
    try:
        with urllib.request.urlopen(url, timeout=timeout_s) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return None


@pytest.fixture(scope="module")
def exllamav2_available() -> dict[str, Any] | None:
    """Probe EXLLAMAV2_HOST for a TabbyAPI / ExLlamaV2 server.

    Returns a dict with `host`, `models`, and a chosen `model` when the
    backend is reachable and reports at least one loaded model. Returns
    None otherwise so live tests skip cleanly.

    Honoured env vars:
      EXLLAMAV2_HOST       — base URL of the server (e.g.
                             `http://127.0.0.1:5000`). When unset the
                             fixture returns None.
      EXLLAMAV2_LIVE_MODEL — pin a specific model id from /v1/models.
                             When unset, the first listed model wins.
    """
    host = os.environ.get("EXLLAMAV2_HOST")
    if not host:
        return None

    body = _http_get_json(f"{host.rstrip('/')}/v1/models", timeout_s=2.0)
    if body is None or not isinstance(body.get("data"), list):
        return None

    models = [m.get("id", "") for m in body["data"] if isinstance(m, dict)]
    models = [m for m in models if m]
    if not models:
        return None

    preferred = os.environ.get("EXLLAMAV2_LIVE_MODEL")
    model = preferred if preferred in models else models[0]
    return {"host": host, "models": models, "model": model}


@pytest.fixture(autouse=True)
def require_live_exllamav2(
    exllamav2_available: dict[str, Any] | None,
) -> dict[str, Any]:
    """Module-level skip helper. Every test in this file uses it."""
    if exllamav2_available is None:
        pytest.skip(
            "EXLLAMAV2_HOST not set or backend unreachable — "
            "set EXLLAMAV2_HOST=http://127.0.0.1:5000 against a running "
            "TabbyAPI / ExLlamaV2 server with at least one loaded model.",
        )
    return exllamav2_available


# ─── Helpers ────────────────────────────────────────────────────────────────


def _host_to_parts(host: str) -> tuple[str, int]:
    """Split `http://h:p` into (h, p). Defaults to port 5000."""
    stripped = host.replace("http://", "").replace("https://", "").rstrip("/")
    if ":" in stripped:
        h, p = stripped.split(":", 1)
        return h, int(p)
    return stripped, 5000


async def _adapter_for(target: dict[str, Any]) -> ExLlamaV2Adapter:
    """Build and initialize an ExLlamaV2Adapter against the live backend."""
    h, p = _host_to_parts(target["host"])
    adapter = ExLlamaV2Adapter()
    config_dict = {
        "host": h,
        "port": p,
        "default_model": target["model"],
        # Keep timeouts modest so a wedged server fails fast in CI rather
        # than blocking the whole live suite.
        "timeout_ms": 30000,
        "stream_timeout_ms": 60000,
    }
    await adapter.initialize(config_dict, hil_handle=None)
    return adapter


# ─── Tests ──────────────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """Adapter initializes against a real server: handle string is
    well-formed, capabilities report streaming, and the discovered
    model list is non-empty (the gate fixture already guaranteed at
    least one loaded model)."""
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        assert adapter._initialized is True
        assert adapter._client is not None
        assert require_live_exllamav2["model"] in adapter._loaded_models
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_hot_swap is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_deterministic(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """temperature=0 yields deterministic output for the same prompt.

    Uses the non-streaming `generate(stream=False)` path — the default
    when callers do not opt into streaming. Asserts two runs produce
    identical text AND that the GenerationResult shape is honoured.
    """
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=32,
        )
        prompt = "Say OK."

        result_a = await adapter.generate(prompt, params, stream=False)
        result_b = await adapter.generate(prompt, params, stream=False)

        assert isinstance(result_a, GenerationResult)
        assert isinstance(result_b, GenerationResult)
        assert len(result_a.text.strip()) > 0
        assert result_a.text == result_b.text, (
            f"Determinism violated: run-1={result_a.text!r}  "
            f"run-2={result_b.text!r}"
        )
        assert result_a.tokens_generated >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_stream_yields_tokens(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """generate(stream=True) returns an AsyncIterator over Token objects;
    verify at least one non-empty chunk arrives, indices are
    monotonically non-decreasing, and the final token is marked
    is_end_of_text per §Generation And Streaming Contract."""
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=32,
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
        assert tokens[-1].is_end_of_text is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_capabilities_match_live_behavior(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """capabilities() must be truthful for the live backend per
    §Capability Truthfulness — streaming and batching are claimed,
    embeddings are NOT, and the claim must match the live path."""
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_embedding is False

        # Truthful embedding=False is proven by the negative path.
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])

        # Truthful streaming=True is proven by the live stream test
        # above; here we re-prove that a stream actually yields.
        params = GenerationParams(temperature=0.0, max_tokens=8)
        stream = await adapter.generate("Say OK.", params, stream=True)
        n = 0
        async for _tok in stream:
            n += 1
        assert n >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_batch_preserves_order(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """generate_batch must preserve input order per §Batch Contract.

    We cannot assert content equality against an unknown live model,
    but cardinality + per-element shape must hold.
    """
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        params = GenerationParams(temperature=0.0, max_tokens=16)
        prompts = [
            "Respond with exactly: ONE",
            "Respond with exactly: TWO",
            "Respond with exactly: THREE",
        ]
        results = await adapter.generate_batch(prompts, params)
        assert len(results) == len(prompts)
        for r in results:
            assert isinstance(r, GenerationResult)
            assert len(r.text) > 0
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_against_real_server(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """health_check() against a running server returns HEALTHY with a
    sensible request counter after a single generate() call."""
    adapter = await _adapter_for(require_live_exllamav2)
    try:
        params = GenerationParams(temperature=0.0, max_tokens=8)
        result = await adapter.generate("Say OK.", params, stream=False)
        assert isinstance(result, GenerationResult)

        status: HealthStatus = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0
        assert status.requests_served >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_shutdown_idempotent(
    require_live_exllamav2: dict[str, Any],
) -> None:
    """Calling shutdown twice is a no-op the second time and leaves the
    adapter in a clean uninitialized state, per §Lifecycle Contract."""
    adapter = await _adapter_for(require_live_exllamav2)

    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None

    # Second call must not raise.
    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
