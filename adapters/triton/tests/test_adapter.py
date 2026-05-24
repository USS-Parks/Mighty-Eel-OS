"""Unit tests for ``adapters.triton.adapter.TritonAdapter``.

Mocks live at the ``TritonClient`` boundary (the backend boundary) per
``docs/ADAPTER-TEST-HARNESS-LOCK.md``: we do not mock the adapter's
own methods. The fake client mimics the real ``TritonClient`` surface
so that the lifecycle, error mapping, capability honesty, and shutdown
behaviour are exercised end-to-end inside the adapter.
"""

from __future__ import annotations

from collections.abc import AsyncIterator
from typing import Any

import pytest

from adapters.base import (
    AdapterCapabilities,
    BackendUnavailableError,
    GenerationParams,
    GenerationResult,
    HealthStatusKind,
    Token,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.triton.adapter import TritonAdapter
from adapters.triton.client import InferResponse

# ─── Fake client at the backend boundary ──────────────────────────────────


class _FakeClient:
    """Minimal stand-in for ``TritonClient``.

    The adapter only sees this object via injection. The fake records
    every call so tests can assert on pooling, ordering, and shutdown.
    """

    def __init__(self) -> None:
        self.server_live_returns: bool = True
        self.server_ready_returns: bool = True
        self.model_ready_returns: bool = True
        self.model_ready_seq: list[bool] | None = None
        self.infer_body: dict[str, Any] = {
            "outputs": [{
                "name": "text_output",
                "shape": [1],
                "datatype": "BYTES",
                "data": ["hello"],
            }],
        }
        self.infer_raises: BaseException | None = None
        self.calls: list[tuple[str, tuple[Any, ...]]] = []
        self.closed: bool = False

    def server_live(self) -> bool:
        self.calls.append(("server_live", ()))
        return self.server_live_returns

    def server_ready(self) -> bool:
        self.calls.append(("server_ready", ()))
        return self.server_ready_returns

    def model_ready(self, path: str) -> bool:
        self.calls.append(("model_ready", (path,)))
        if self.model_ready_seq is not None:
            if self.model_ready_seq:
                return self.model_ready_seq.pop(0)
            return False
        return self.model_ready_returns

    def infer(
        self,
        model_path: str,
        inputs: list[dict[str, Any]],
        outputs: list[dict[str, Any]] | None = None,
        *,
        model_hint: str | None = None,
    ) -> InferResponse:
        self.calls.append(
            ("infer", (model_path, inputs, outputs, model_hint)),
        )
        if self.infer_raises is not None:
            raise self.infer_raises
        return InferResponse(
            status_code=200, body=self.infer_body, elapsed_ms=1.0,
        )

    def close(self) -> None:
        self.calls.append(("close", ()))
        self.closed = True


# ─── Helpers ──────────────────────────────────────────────────────────────


def _text_io_config(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "host": "127.0.0.1",
        "port": 8000,
        "model_name": "ensemble",
        "input_tensor_name": "text_input",
        "output_tensor_name": "text_output",
        "input_datatype": "BYTES",
        "output_datatype": "BYTES",
    }
    base.update(overrides)
    return base


async def _init_with_fake(
    adapter: TritonAdapter, fake: _FakeClient, config: dict[str, Any],
) -> None:
    """Initialize ``adapter`` against a pre-injected fake client."""
    # Inject the fake first so initialize() does not create a real client.
    adapter._client = fake
    await adapter.initialize(config)


# ─── Construction ────────────────────────────────────────────────────────


class TestConstruction:
    def test_init_stores_config_no_network(self) -> None:
        adapter = TritonAdapter(_text_io_config(host="boom.invalid"))
        # No client yet, no init -> no network attempted.
        assert adapter._client is None
        assert adapter._initialized is False
        assert adapter._tconfig.model_name == "ensemble"


# ─── Initialize ──────────────────────────────────────────────────────────


class TestInitialize:
    @pytest.mark.asyncio
    async def test_happy_path(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        assert adapter._initialized is True
        assert adapter._model_ready is True
        assert ("server_live", ()) in fake.calls
        assert ("model_ready", ("/v2/models/ensemble",)) in fake.calls

    @pytest.mark.asyncio
    async def test_missing_model_name_raises_validation(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        adapter._client = fake
        with pytest.raises(ValidationError):
            await adapter.initialize({"model_name": ""})

    @pytest.mark.asyncio
    async def test_server_not_live_raises_unavailable(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        fake.server_live_returns = False
        adapter._client = fake
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize(_text_io_config())
        # Half-open client released so a retry can re-initialize cleanly.
        assert fake.closed is True
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_polls_model_ready_until_ready(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        fake.model_ready_seq = [False, True]
        await _init_with_fake(
            adapter, fake,
            _text_io_config(
                readiness_poll_attempts=3,
                readiness_poll_interval_ms=0,
            ),
        )
        assert adapter._model_ready is True
        ready_calls = [c for c in fake.calls if c[0] == "model_ready"]
        assert len(ready_calls) == 2

    @pytest.mark.asyncio
    async def test_initialize_degrades_when_model_never_ready(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        fake.model_ready_returns = False
        await _init_with_fake(adapter, fake, _text_io_config())
        # Initialize still succeeds; health_check will report degraded.
        assert adapter._initialized is True
        assert adapter._model_ready is False


# ─── Generate (non-streaming) ────────────────────────────────────────────


class TestGenerateNonStreaming:
    @pytest.mark.asyncio
    async def test_returns_generation_result(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        out = await adapter.generate("hi", GenerationParams(), stream=False)
        assert isinstance(out, GenerationResult)
        assert out.text == "hello"
        assert out.tokens_generated >= 1
        assert out.finish_reason.value == "stop"

    @pytest.mark.asyncio
    async def test_input_tensor_uses_configured_name(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config(
            input_tensor_name="TEXT_IN",
            output_tensor_name="TEXT_OUT",
        ))
        fake.infer_body = {
            "outputs": [{
                "name": "TEXT_OUT",
                "shape": [1],
                "datatype": "BYTES",
                "data": ["ok"],
            }],
        }
        out = await adapter.generate("hello", GenerationParams(), stream=False)
        assert isinstance(out, GenerationResult)
        assert out.text == "ok"
        infer_call = next(c for c in fake.calls if c[0] == "infer")
        _path, inputs, outputs, _hint = infer_call[1]
        assert inputs[0]["name"] == "TEXT_IN"
        assert outputs[0]["name"] == "TEXT_OUT"

    @pytest.mark.asyncio
    async def test_unsupported_when_text_io_not_wired(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, {
            "model_name": "yolo",
            # input/output tensor names left blank => unsupported
        })
        with pytest.raises(UnsupportedOperationError):
            await adapter.generate("hi", GenerationParams(), stream=False)

    @pytest.mark.asyncio
    async def test_pre_initialize_raises_unavailable(self) -> None:
        adapter = TritonAdapter(_text_io_config())
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams(), stream=False)


# ─── Generate streaming ─────────────────────────────────────────────────


class TestGenerateStreaming:
    @pytest.mark.asyncio
    async def test_single_token_frame(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        stream = await adapter.generate("hi", GenerationParams(), stream=True)
        assert isinstance(stream, AsyncIterator)
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)
        # KServe v2 /infer is unary -> one Token, marked end-of-text.
        assert len(tokens) == 1
        assert tokens[0].text == "hello"
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_capabilities_report_streaming_false(self) -> None:
        adapter = TritonAdapter(_text_io_config())
        # Capabilities are static — no init required.
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is False


# ─── Generate batch ─────────────────────────────────────────────────────


class TestGenerateBatch:
    @pytest.mark.asyncio
    async def test_preserves_input_order(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        fake.infer_body = {
            "outputs": [{
                "name": "text_output",
                "shape": [3],
                "datatype": "BYTES",
                "data": ["a", "b", "c"],
            }],
        }
        await _init_with_fake(adapter, fake, _text_io_config())
        results = await adapter.generate_batch(
            ["one", "two", "three"], GenerationParams(),
        )
        assert [r.text for r in results] == ["a", "b", "c"]

    @pytest.mark.asyncio
    async def test_empty_input_returns_empty(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        results = await adapter.generate_batch([], GenerationParams())
        assert results == []
        # No infer call was made for the empty batch.
        assert all(c[0] != "infer" for c in fake.calls)

    @pytest.mark.asyncio
    async def test_pads_when_backend_returns_too_few(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        fake.infer_body = {
            "outputs": [{
                "name": "text_output",
                "shape": [1],
                "datatype": "BYTES",
                "data": ["only-one"],
            }],
        }
        await _init_with_fake(adapter, fake, _text_io_config())
        results = await adapter.generate_batch(
            ["one", "two", "three"], GenerationParams(),
        )
        assert [r.text for r in results] == ["only-one", "", ""]


# ─── Embed ───────────────────────────────────────────────────────────────


class TestEmbed:
    @pytest.mark.asyncio
    async def test_unsupported_by_default(self) -> None:
        adapter = TritonAdapter()
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hi"])


# ─── Raw infer surface ──────────────────────────────────────────────────


class TestRawInfer:
    @pytest.mark.asyncio
    async def test_proxies_tensors_through(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, {"model_name": "yolo"})
        body = await adapter.infer(
            [{"name": "img", "shape": [1, 3, 224, 224],
              "datatype": "FP32", "data": [0.0]}],
            outputs=[{"name": "scores"}],
        )
        assert "outputs" in body
        infer_call = next(c for c in fake.calls if c[0] == "infer")
        _path, inputs, outputs, hint = infer_call[1]
        assert inputs[0]["name"] == "img"
        assert outputs[0]["name"] == "scores"
        assert hint == "yolo"

    @pytest.mark.asyncio
    async def test_pre_initialize_raises_unavailable(self) -> None:
        adapter = TritonAdapter({"model_name": "yolo"})
        with pytest.raises(BackendUnavailableError):
            await adapter.infer(
                [{"name": "x", "shape": [1],
                  "datatype": "BYTES", "data": ["x"]}],
            )


# ─── Health ─────────────────────────────────────────────────────────────


class TestHealth:
    @pytest.mark.asyncio
    async def test_unavailable_before_initialize(self) -> None:
        adapter = TritonAdapter(_text_io_config())
        status = await adapter.health_check()
        assert status.kind is HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_healthy_when_server_and_model_ready(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        status = await adapter.health_check()
        assert status.kind is HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0

    @pytest.mark.asyncio
    async def test_degraded_when_model_not_ready(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        fake.model_ready_returns = False
        status = await adapter.health_check()
        assert status.kind is HealthStatusKind.DEGRADED
        assert "not ready" in (status.reason or "")

    @pytest.mark.asyncio
    async def test_unavailable_when_server_not_ready(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        fake.server_ready_returns = False
        status = await adapter.health_check()
        assert status.kind is HealthStatusKind.UNAVAILABLE


# ─── Capabilities ───────────────────────────────────────────────────────


class TestCapabilities:
    def test_text_io_off_means_zero_context(self) -> None:
        adapter = TritonAdapter({"model_name": "yolo"})
        caps = adapter.capabilities()
        assert caps.max_context_window == 0
        assert caps.supports_batching is False
        assert caps.supports_embedding is False
        assert caps.supports_streaming is False
        assert caps.backend_version == "kserve-v2"
        assert caps.extra["text_io"] is False

    def test_text_io_on_reports_max_context(self) -> None:
        adapter = TritonAdapter(_text_io_config(max_input_len=2048))
        caps = adapter.capabilities()
        assert caps.max_context_window == 2048
        assert caps.supports_batching is True
        assert caps.extra["text_io"] is True

    def test_embedding_flag_is_truthful(self) -> None:
        adapter = TritonAdapter({
            "model_name": "embed",
            "declares_embedding": True,
        })
        caps = adapter.capabilities()
        assert caps.supports_embedding is True


# ─── Shutdown ───────────────────────────────────────────────────────────


class TestShutdown:
    @pytest.mark.asyncio
    async def test_closes_client(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        await adapter.shutdown()
        assert fake.closed is True
        assert adapter._client is None
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_double_shutdown_is_safe(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        await adapter.shutdown()
        await adapter.shutdown()  # must not raise

    @pytest.mark.asyncio
    async def test_post_shutdown_call_raises_unavailable(self) -> None:
        adapter = TritonAdapter()
        fake = _FakeClient()
        await _init_with_fake(adapter, fake, _text_io_config())
        await adapter.shutdown()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams(), stream=False)


# ─── Registry ───────────────────────────────────────────────────────────


def test_adapter_is_registered() -> None:
    from adapters.base import get_adapter
    cls = get_adapter("triton")
    assert cls is TritonAdapter
    assert getattr(cls, "_mai_adapter_version", "") == "1.0.0"
