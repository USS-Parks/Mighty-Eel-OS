"""Live-backend integration tests for the generic Triton adapter.

These tests hit a REAL NVIDIA Triton Inference Server over the KServe
v2 HTTP protocol. They SKIP cleanly when the gate env vars are unset
or the server is unreachable, so the default ``pytest`` command still
runs the mocked unit tests in ``test_adapter.py`` without error on a
machine that has no Triton installed.

Opt-in:

    # Start Triton in another terminal first, e.g.:
    docker run --rm --gpus=all -p 8000:8000 -p 8001:8001 \\
        -v /path/to/model_repository:/models \\
        nvcr.io/nvidia/tritonserver:24.04-py3 \\
        tritonserver --model-repository=/models

    # Then:
    export TRITON_HOST=http://127.0.0.1:8000
    export TRITON_MODEL_NAME=ensemble_simple        # or your model
    # Optional, to exercise the text generate() surface:
    export TRITON_INPUT_TENSOR=text_input
    export TRITON_OUTPUT_TENSOR=text_output
    pytest -m live_backend adapters/triton/tests/test_integration_live.py -v

DOUGHERTY lane J-26. The live-test gate variable is ``TRITON_HOST``
per ``docs/ADAPTER-TEST-HARNESS-LOCK.md``.
"""

from __future__ import annotations

from typing import Any
from urllib.parse import urlparse

import pytest

from adapters.base import (
    AdapterCapabilities,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    HealthStatusKind,
    UnsupportedOperationError,
)
from adapters.triton.adapter import TritonAdapter

pytestmark = pytest.mark.live_backend


# ─── Helpers ────────────────────────────────────────────────────────────


def _host_to_parts(host: str) -> tuple[str, int, bool]:
    """Split ``http(s)://h:p`` into (host, port, use_ssl). Default port 8000."""
    parsed = urlparse(host if "://" in host else f"http://{host}")
    use_ssl = (parsed.scheme == "https")
    hostname = parsed.hostname or "127.0.0.1"
    port = parsed.port or (443 if use_ssl else 8000)
    return hostname, port, use_ssl


async def _adapter_for(target: dict[str, Any]) -> TritonAdapter:
    host, port, use_ssl = _host_to_parts(target["host"])
    config: dict[str, Any] = {
        "host": host,
        "port": port,
        "use_ssl": use_ssl,
        "model_name": target["model"],
        "model_version": target.get("version") or "",
        "timeout_ms": 30000,
        "stream_timeout_ms": 30000,
    }
    if target.get("input_tensor") and target.get("output_tensor"):
        config["input_tensor_name"] = target["input_tensor"]
        config["output_tensor_name"] = target["output_tensor"]
        config["input_datatype"] = "BYTES"
        config["output_datatype"] = "BYTES"
    adapter = TritonAdapter()
    await adapter.initialize(config, hil_handle=None)
    return adapter


# ─── Skip guard ─────────────────────────────────────────────────────────


@pytest.fixture(autouse=True)
def require_live_triton(
    triton_available: dict[str, Any] | None,
) -> dict[str, Any]:
    if triton_available is None:
        pytest.skip(
            "TRITON_HOST / TRITON_MODEL_NAME not set or server unreachable — "
            "set TRITON_HOST=http://127.0.0.1:8000 and TRITON_MODEL_NAME=<m> "
            "to enable live tests.",
        )
    return triton_available


# ─── Tests ──────────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_initialize_against_real_server(
    require_live_triton: dict[str, Any],
) -> None:
    """Adapter initializes against a real Triton server and reports honest
    capabilities (streaming False; batching gated on text-IO wiring)."""
    adapter = await _adapter_for(require_live_triton)
    try:
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is False
        assert caps.backend_version == "kserve-v2"
        assert caps.extra["model_name"] == require_live_triton["model"]
        assert adapter._initialized is True
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_check_against_real_server(
    require_live_triton: dict[str, Any],
) -> None:
    adapter = await _adapter_for(require_live_triton)
    try:
        status = await adapter.health_check()
        assert isinstance(status, HealthStatus)
        # Server is reachable (otherwise the autouse fixture would have
        # skipped); model may be healthy or degraded but never unavailable.
        assert status.kind in (
            HealthStatusKind.HEALTHY, HealthStatusKind.DEGRADED,
        )
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_generate_text_round_trip_when_wired(
    require_live_triton: dict[str, Any],
) -> None:
    """When TRITON_INPUT_TENSOR / TRITON_OUTPUT_TENSOR are supplied,
    generate() must return a non-empty GenerationResult against the
    real server. When they are not, the test skips."""
    if not require_live_triton.get("input_tensor"):
        pytest.skip(
            "TRITON_INPUT_TENSOR / TRITON_OUTPUT_TENSOR not set — "
            "text-IO live test skipped.",
        )
    adapter = await _adapter_for(require_live_triton)
    try:
        params = GenerationParams(temperature=0.0, top_p=1.0, max_tokens=64)
        result = await adapter.generate("Say OK.", params, stream=False)
        assert isinstance(result, GenerationResult)
        assert result.tokens_generated >= 1
        # Real server response is non-empty even with a tiny model.
        assert isinstance(result.text, str)
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_embed_remains_unsupported(
    require_live_triton: dict[str, Any],
) -> None:
    """Live or not, the high-level embed() always raises -- generic
    Triton serves embedding models via the raw infer() surface."""
    adapter = await _adapter_for(require_live_triton)
    try:
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hi"])
    finally:
        await adapter.shutdown()


@pytest.mark.asyncio
async def test_capabilities_match_live_wiring(
    require_live_triton: dict[str, Any],
) -> None:
    """Capability flags reflect what the operator actually wired up."""
    adapter = await _adapter_for(require_live_triton)
    try:
        caps = adapter.capabilities()
        text_io_wired = bool(require_live_triton.get("input_tensor"))
        assert caps.extra["text_io"] is text_io_wired
        if text_io_wired:
            assert caps.max_context_window > 0
            assert caps.supports_batching is True
        else:
            assert caps.max_context_window == 0
            assert caps.supports_batching is False
    finally:
        await adapter.shutdown()
