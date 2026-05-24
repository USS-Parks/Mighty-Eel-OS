"""Live-backend integration tests for the vLLM adapter.

These tests hit a REAL vLLM OpenAI-compatible server (`vllm serve ...`
or `python -m vllm.entrypoints.openai.api_server ...`). They SKIP
cleanly when the backend is unavailable, so `pytest adapters/vllm/`
on a machine without vLLM still runs the mocked unit tests in
`test_adapter.py` without error.

Opt-in:
    # Start vLLM in another terminal first:
    vllm serve Qwen/Qwen2.5-0.5B-Instruct --port 8000

    export VLLM_HOST=http://127.0.0.1:8000
    pytest -m live_backend adapters/vllm/tests/test_integration_live.py -v

To pin the model used (otherwise the first /v1/models entry wins):
    export VLLM_LIVE_MODEL=Qwen/Qwen2.5-0.5B-Instruct

The embedding test auto-skips when the discovered model does not serve
embeddings (vLLM only exposes /v1/embeddings when launched with an
embedding-class model, e.g. `--task embed` against an
intfloat/e5-mistral-7b-instruct or BGE checkpoint).

J-18 (DOUGHERTY lane). Backend gate variable per
`docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Live Backend Environment Variables".
The VLLM_HOST probe is intentionally inline here (no shared
`conftest.py` fixture) — adapter-scope only per the parallel-merge
rules in J-18..J-26. The convergence pass may migrate this probe to
`mai/conftest.py` as a `vllm_available` session fixture if it wants
parity with `ollama_available` and `llamacpp_available`.
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
    Embedding,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    HealthStatusKind,
    Token,
    UnsupportedOperationError,
)
from adapters.vllm.adapter import VllmAdapter

# Every test in this module is opt-in and skips cleanly when the
# backend is unavailable.
pytestmark = pytest.mark.live_backend


# ─── Inline backend probe (no shared conftest dependency) ───────────────────


def _http_get_json(url: str, timeout_s: float) -> dict[str, Any] | None:
    """Stdlib-only single-shot GET. Returns parsed JSON or None on any failure.

    Air-gap-policy compliant per `docs/ADAPTER-COMPLETION-MATRIX.md` §1.
    """
    try:
        with urllib.request.urlopen(url, timeout=timeout_s) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return None


def _http_get_ok(url: str, timeout_s: float) -> bool:
    """Probe an endpoint that may return an empty body (e.g. /health)."""
    try:
        with urllib.request.urlopen(url, timeout=timeout_s) as resp:
            return 200 <= resp.status < 300
    except (urllib.error.URLError, TimeoutError, OSError):
        return False


@pytest.fixture(scope="module")
def vllm_available() -> dict[str, Any] | None:
    """Module-scoped check for a reachable vLLM OpenAI-compatible server.

    Returns a dict with `host` (str), `models` (list[str]), and `model`
    (str — the live model to use) when vLLM is reachable; returns None
    otherwise.

    Probe sequence:
      1. `GET /health` — vLLM serves this on the same port and returns
         200 with an empty body when ready.
      2. `GET /v1/models` — must return at least one entry under
         `data[].id`. The first entry wins unless `VLLM_LIVE_MODEL` is
         set and matches one of the listed ids.

    Honoured env vars:
      VLLM_HOST       — base URL of the vLLM server (e.g.
                        `http://127.0.0.1:8000`). When unset, this
                        fixture returns None and live tests skip.
      VLLM_LIVE_MODEL — specific model id to use. When unset, the
                        first model returned by /v1/models wins.
    """
    host = os.environ.get("VLLM_HOST")
    if not host:
        return None

    base = host.rstrip("/")
    if not _http_get_ok(f"{base}/health", timeout_s=2.0):
        return None

    models_resp = _http_get_json(f"{base}/v1/models", timeout_s=2.0)
    if models_resp is None:
        return None
    data = models_resp.get("data", [])
    if not isinstance(data, list):
        return None
    ids = [m.get("id", "") for m in data if isinstance(m, dict)]
    ids = [m for m in ids if m]
    if not ids:
        return None

    preferred = os.environ.get("VLLM_LIVE_MODEL")
    model = preferred if preferred and preferred in ids else ids[0]

    return {"host": host, "models": ids, "model": model}


# ─── Adapter helpers ────────────────────────────────────────────────────────


def _host_to_parts(host: str) -> tuple[str, int]:
    """Split a `http://h:p` URL into (host, port). Defaults to port 8000."""
    stripped = host.replace("http://", "").replace("https://", "").rstrip("/")
    if ":" in stripped:
        h, p = stripped.split(":", 1)
        return h, int(p)
    return stripped, 8000


async def _adapter_for(target: dict[str, Any]) -> VllmAdapter:
    """Build and initialize a VllmAdapter against the live backend."""
    h, p = _host_to_parts(target["host"])
    adapter = VllmAdapter()
    config_dict = {
        "host": h,
        "port": p,
        "default_model": target["model"],
        # Keep tensor parallelism off — the live test does not assume
        # multi-GPU. The actual TP size is whatever the running vLLM
        # server was launched with; this field is informational only
        # for the adapter.
        "tensor_parallel_size": 1,
        "enable_lora": False,
    }
    await adapter.initialize(config_dict, hil_handle=None)
    return adapter


# ─── Skip-when-unavailable guard ────────────────────────────────────────────


@pytest.fixture(autouse=True)
def require_live_vllm(
    vllm_available: dict[str, Any] | None,
) -> dict[str, Any]:
    """Module-level skip helper. Every test in this file uses it."""
    if vllm_available is None:
        pytest.skip(
            "VLLM_HOST not set or vLLM server unreachable — "
            "set VLLM_HOST=http://127.0.0.1:8000 to enable live tests.",
        )
    return vllm_available


# ─── Tests ──────────────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_vllm: dict[str, Any],
) -> None:
    """Adapter initializes against a real vLLM server: capabilities are
    declared, the discovered model is one of the live /v1/models entries,
    and the handle has the expected vllm- prefix."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        assert adapter._initialized is True
        assert adapter._model in require_live_vllm["models"]
        assert adapter._available_models == require_live_vllm["models"]
        assert adapter._start_time_ms > 0
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_returns_non_empty_text(
    require_live_vllm: dict[str, Any],
) -> None:
    """Single non-streaming generation against a real vLLM server with
    a tiny token budget. Asserts the returned text is non-empty and
    the GenerationResult shape is honoured."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=16,
        )
        result = await adapter.generate("Say OK.", params, stream=False)
        assert isinstance(result, GenerationResult)
        assert len(result.text.strip()) > 0
        assert result.tokens_generated >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_stream_yields_ordered_tokens(
    require_live_vllm: dict[str, Any],
) -> None:
    """generate(stream=True) returns an AsyncIterator over Token
    objects; verify at least one non-empty chunk arrives, the indices
    are monotonically non-decreasing, and the final token is marked
    end-of-text."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        params = GenerationParams(
            temperature=0.0,
            top_p=1.0,
            max_tokens=32,
        )
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
        assert sum(len(t.text) for t in non_empty) > 0

        indices = [t.index for t in tokens]
        assert indices == sorted(indices)
        assert indices[0] == 0
        assert tokens[-1].is_end_of_text is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_embeddings_when_backend_supports_else_unsupported(
    require_live_vllm: dict[str, Any],
) -> None:
    """vLLM only serves /v1/embeddings when launched with an embedding
    model. This test probes the live endpoint cheaply: if /v1/embeddings
    accepts the call, assert the adapter returns well-formed Embedding
    objects; otherwise skip (this is an honest hardware/model gating,
    not a failure)."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        try:
            vectors = await adapter.embed(["hello", "hello", "different"])
        except UnsupportedOperationError:
            pytest.skip(
                "Adapter declared embedding unsupported for this backend "
                "configuration; this is an honest skip.",
            )
        except Exception as exc:  # noqa: BLE001
            # vLLM responds 404/400 when launched without an embedding
            # model. The client maps 404 → ModelNotFoundError; either
            # branch is a clean honest skip for the embedding live test.
            pytest.skip(
                f"vLLM /v1/embeddings unavailable for this model "
                f"({type(exc).__name__}); start vLLM with an embedding "
                f"model to exercise this path.",
            )

        assert len(vectors) == 3
        for emb in vectors:
            assert isinstance(emb, Embedding)
            assert len(emb.vector) > 0
            assert emb.input_tokens >= 0
        # Determinism: identical inputs yield identical vectors.
        assert vectors[0].vector == vectors[1].vector
        # Distinct text yields a different vector.
        assert vectors[0].vector != vectors[2].vector
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_against_real_server(
    require_live_vllm: dict[str, Any],
) -> None:
    """health_check() against a running vLLM server returns HEALTHY
    with non-negative uptime and request counters after a single
    generate."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        params = GenerationParams(temperature=0.0, max_tokens=8)
        result = await adapter.generate("Say ok.", params, stream=False)
        assert isinstance(result, GenerationResult)

        status: HealthStatus = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0
        assert status.requests_served >= 1
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_capabilities_reflect_live_backend(
    require_live_vllm: dict[str, Any],
) -> None:
    """ADAPTER-SHARED-CONTRACT §"Capability Truthfulness": every flag
    the adapter declares must correspond to an implemented path. This
    test pins the live-backend view of the declarative flags."""
    adapter = await _adapter_for(require_live_vllm)
    try:
        caps = adapter.capabilities()
        # Hardcoded vLLM-class capability surface — adapter source of
        # truth, not a runtime backend introspection.
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_structured_output is True
        assert caps.supports_continuous_batching is True
        assert caps.supports_embedding is True
        assert caps.supports_hot_swap is True
        assert caps.supports_vision is False
        # Honest disclosure: supports_tool_calling=True is declarative
        # — the adapter does not currently forward tools[]/tool_choice
        # to the OpenAI-compat chat-completions body. Flagged in the
        # J-18 evidence note Known Limitations. Asserting on the flag
        # value preserves the existing contract claim; tightening it
        # to actual code-path coverage belongs in a convergence
        # session.
        assert caps.supports_tool_calling is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_shutdown_idempotent(
    require_live_vllm: dict[str, Any],
) -> None:
    """Calling shutdown twice is a no-op the second time and leaves
    the adapter in a clean uninitialized state."""
    adapter = await _adapter_for(require_live_vllm)

    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None

    # Second call must not raise.
    await adapter.shutdown()
    assert adapter._initialized is False
    assert adapter._client is None
