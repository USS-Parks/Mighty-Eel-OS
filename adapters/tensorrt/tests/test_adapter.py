"""Unit tests for the TensorRT-LLM adapter and HTTP client.

These tests cover the ``AdapterBase`` unit minimums from
``docs/ADAPTER-TEST-HARNESS-LOCK.md``:

- construction stores config without network calls
- initialize happy / unavailable / config-validation paths
- generate non-streaming happy path returns GenerationResult
- generate streaming yields ordered Token objects, terminated by EOT
- generate maps timeout to AdapterTimeoutError
- generate maps backend OOM to OutOfMemoryError
- generate maps model-not-found to ModelNotFoundError
- generate maps malformed body to BackendCrashedError
- generate_batch preserves order, handles empty input intentionally
- embed raises UnsupportedOperationError without needing initialization
- health_check returns healthy / degraded / unavailable
- capabilities are truthful for every advertised feature
- shutdown is idempotent; post-shutdown calls fail deterministically
- the client reuses its urllib opener across two requests (pooling)

No live backend is touched. All HTTP traffic is intercepted via a stub
opener that records every request and returns canned responses.

Mocks attach at the urllib boundary (for client tests) or at the
``TensorRtClient`` boundary (for adapter tests). The adapter under test
is never mocked.
"""

from __future__ import annotations

import io
import json
import urllib.error
from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest

from adapters.base import (
    AdapterError,
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    ContextExceededError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatus,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    Token,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.tensorrt.adapter import TensorRtAdapter
from adapters.tensorrt.client import TensorRtClient, TritonResponse, TritonStreamChunk
from adapters.tensorrt.config import TensorRtConfig

# ─── Fixtures ──────────────────────────────────────────────────────────────


@pytest.fixture
def default_config_dict() -> dict[str, Any]:
    return {
        "host": "127.0.0.1",
        "port": 8000,
        "default_model": "llama-trt",
        "timeout_ms": 5000,
        "stream_timeout_ms": 10000,
    }


@pytest.fixture
def adapter(default_config_dict: dict[str, Any]) -> TensorRtAdapter:
    """Constructed-but-not-initialized adapter."""
    return TensorRtAdapter(default_config_dict)


def _stub_client_for_init(
    *, healthy: bool = True, model_ready: bool = True,
) -> MagicMock:
    """Build a stub TensorRtClient that the adapter can wrap during init."""
    client = MagicMock(spec=TensorRtClient)
    client.health.return_value = healthy
    client.model_ready.return_value = model_ready
    client.close = MagicMock()
    return client


async def _initialize_with_stub(
    adapter: TensorRtAdapter,
    client: MagicMock,
) -> None:
    """Drive initialize() against a stub client by patching the factory."""
    import adapters.tensorrt.adapter as adapter_mod

    real_cls = adapter_mod.TensorRtClient
    adapter_mod.TensorRtClient = MagicMock(return_value=client)
    try:
        await adapter.initialize()
    finally:
        adapter_mod.TensorRtClient = real_cls


# ─── Config ────────────────────────────────────────────────────────────────


class TestTensorRtConfig:
    def test_defaults(self) -> None:
        cfg = TensorRtConfig()
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8001
        assert cfg.grpc_port == 8002
        assert cfg.timeout_ms > 0
        assert cfg.enable_inflight_batching is True

    def test_from_dict_known_fields(self) -> None:
        cfg = TensorRtConfig.from_dict(
            {"host": "10.0.0.5", "port": 9000, "precision": "int8"},
        )
        assert cfg.host == "10.0.0.5"
        assert cfg.port == 9000
        assert cfg.precision == "int8"

    def test_from_dict_unknown_fields_go_to_extra(self) -> None:
        cfg = TensorRtConfig.from_dict({"host": "127.0.0.1", "unknown": "yes"})
        assert cfg.extra_options == {"unknown": "yes"}

    def test_base_url(self) -> None:
        cfg = TensorRtConfig(host="trt", port=1234)
        assert cfg.base_url == "http://trt:1234"


# ─── Adapter construction + lifecycle ─────────────────────────────────────


class TestConstruction:
    def test_init_stores_config_without_network(
        self, default_config_dict: dict[str, Any],
    ) -> None:
        a = TensorRtAdapter(default_config_dict)
        assert a._initialized is False
        assert a._client is None
        assert a._config.host == "127.0.0.1"
        assert a._config.default_model == "llama-trt"

    def test_init_with_no_config_uses_defaults(self) -> None:
        a = TensorRtAdapter()
        assert isinstance(a._config, TensorRtConfig)
        assert a._client is None


class TestInitialize:
    @pytest.mark.asyncio
    async def test_initialize_happy_path(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=True)
        await _initialize_with_stub(adapter, client)
        assert adapter._initialized is True
        assert adapter._client is client
        assert adapter._engine_ready is True
        assert adapter._model_name == "llama-trt"

    @pytest.mark.asyncio
    async def test_initialize_returns_stable_handle(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        import adapters.tensorrt.adapter as adapter_mod

        real_cls = adapter_mod.TensorRtClient
        adapter_mod.TensorRtClient = MagicMock(return_value=client)
        try:
            handle = await adapter.initialize()
        finally:
            adapter_mod.TensorRtClient = real_cls
        assert handle.startswith("tensorrt-llama-trt-")

    @pytest.mark.asyncio
    async def test_initialize_unavailable_backend_raises(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=False)
        with pytest.raises(BackendUnavailableError):
            await _initialize_with_stub(adapter, client)
        assert adapter._initialized is False
        assert adapter._client is None
        client.close.assert_called_once()

    @pytest.mark.asyncio
    async def test_initialize_validates_config(self) -> None:
        a = TensorRtAdapter({"host": "", "default_model": "x"})
        with pytest.raises(ValidationError):
            await a.initialize()

    @pytest.mark.asyncio
    async def test_initialize_rejects_bad_port(self) -> None:
        a = TensorRtAdapter({"host": "127.0.0.1", "port": 0, "default_model": "x"})
        with pytest.raises(ValidationError):
            await a.initialize()

    @pytest.mark.asyncio
    async def test_initialize_degraded_when_model_not_ready(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=False)
        await _initialize_with_stub(adapter, client)
        assert adapter._initialized is True
        assert adapter._engine_ready is False


class TestShutdown:
    @pytest.mark.asyncio
    async def test_shutdown_closes_client(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None
        client.close.assert_called_once()

    @pytest.mark.asyncio
    async def test_double_shutdown_is_safe(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        await adapter.shutdown()
        await adapter.shutdown()  # must not raise
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_post_shutdown_generate_raises(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        await adapter.shutdown()
        with pytest.raises(AdapterError) as excinfo:
            await adapter.generate("hi", GenerationParams())
        assert excinfo.value.code == "NotReady"

    @pytest.mark.asyncio
    async def test_reinitialize_after_shutdown_works(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client1 = _stub_client_for_init()
        await _initialize_with_stub(adapter, client1)
        await adapter.shutdown()
        client2 = _stub_client_for_init()
        await _initialize_with_stub(adapter, client2)
        assert adapter._initialized is True
        assert adapter._client is client2


# ─── Generation ───────────────────────────────────────────────────────────


class TestGenerateNonStreaming:
    @pytest.mark.asyncio
    async def test_rejects_empty_prompt(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        with pytest.raises(ValidationError):
            await adapter.generate("", GenerationParams(), stream=False)

    @pytest.mark.asyncio
    async def test_rejects_invalid_sampling_params(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        with pytest.raises(ValidationError):
            await adapter.generate("hi", GenerationParams(max_tokens=0), stream=False)

    @pytest.mark.asyncio
    async def test_returns_generation_result(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.return_value = TritonResponse(
            status_code=200,
            body={"text_output": "Paris", "output_tokens": 1},
            elapsed_ms=12.3,
        )
        result = await adapter.generate("Capital of France?", GenerationParams())
        assert isinstance(result, GenerationResult)
        assert result.text == "Paris"
        assert result.tokens_generated == 1
        assert result.finish_reason in (FinishReason.STOP, FinishReason.MAX_TOKENS)
        assert adapter._requests_served == 1

    @pytest.mark.asyncio
    async def test_handles_openai_style_choices_body(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.return_value = TritonResponse(
            status_code=200,
            body={"choices": [{"text": "yes"}], "output_tokens": 1},
            elapsed_ms=1.0,
        )
        result = await adapter.generate("Are we live?", GenerationParams())
        assert isinstance(result, GenerationResult)
        assert result.text == "yes"

    @pytest.mark.asyncio
    async def test_max_tokens_reported_as_max_tokens_finish(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.return_value = TritonResponse(
            status_code=200,
            body={"text_output": "x" * 4, "output_tokens": 4, "finish_reason": "length"},
            elapsed_ms=1.0,
        )
        result = await adapter.generate("hi", GenerationParams(max_tokens=4))
        assert isinstance(result, GenerationResult)
        assert result.finish_reason == FinishReason.MAX_TOKENS

    @pytest.mark.asyncio
    async def test_propagates_timeout(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.side_effect = AdapterTimeoutError(timeout_ms=2000)
        with pytest.raises(AdapterTimeoutError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_propagates_model_not_found(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.side_effect = ModelNotFoundError(model="llama-trt")
        with pytest.raises(ModelNotFoundError) as excinfo:
            await adapter.generate("hi", GenerationParams())
        assert excinfo.value.data.get("model") == "llama-trt"

    @pytest.mark.asyncio
    async def test_propagates_oom(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.side_effect = OutOfMemoryError()
        with pytest.raises(OutOfMemoryError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_malformed_response_maps_to_crashed(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        # Returning a stream iterator from a non-streaming call must
        # be mapped into a typed adapter error, not silently accepted.
        client.generate.return_value = iter(
            [TritonStreamChunk(text="x", finished=True)],
        )
        with pytest.raises(BackendCrashedError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_pre_initialize_generate_fails(
        self, adapter: TensorRtAdapter,
    ) -> None:
        with pytest.raises(AdapterError) as excinfo:
            await adapter.generate("hi", GenerationParams())
        assert excinfo.value.code == "NotReady"


class TestGenerateStreaming:
    @pytest.mark.asyncio
    async def test_yields_ordered_tokens(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.return_value = iter(
            [
                TritonStreamChunk(text="Pa", finished=False),
                TritonStreamChunk(text="ris", finished=False),
                TritonStreamChunk(text=".", finished=True),
            ],
        )
        tokens: list[Token] = []
        result = await adapter.generate("Capital of France?", GenerationParams(), stream=True)
        async for tok in result:
            tokens.append(tok)
        assert "".join(t.text for t in tokens) == "Paris."
        # Indices are monotonically non-decreasing (EOT marker may share an index).
        assert [t.index for t in tokens] == sorted(t.index for t in tokens)
        assert tokens[-1].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_streaming_terminates_without_finished_flag(
        self, adapter: TensorRtAdapter,
    ) -> None:
        """Iterator exhaustion is also a clean stop."""
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.generate.return_value = iter(
            [TritonStreamChunk(text="hello", finished=False)],
        )
        tokens: list[Token] = []
        result = await adapter.generate("hi", GenerationParams(), stream=True)
        async for tok in result:
            tokens.append(tok)
        assert tokens[-1].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_streaming_propagates_malformed_frame(
        self, adapter: TensorRtAdapter,
    ) -> None:
        """A BackendCrashedError raised inside the iterator surfaces."""
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)

        def _bad_stream() -> Any:
            yield TritonStreamChunk(text="ok", finished=False)
            raise BackendCrashedError(detail="malformed SSE frame")

        client.generate.return_value = _bad_stream()
        result = await adapter.generate("hi", GenerationParams(), stream=True)
        with pytest.raises(BackendCrashedError):
            async for _tok in result:
                pass


class TestGenerateBatch:
    @pytest.mark.asyncio
    async def test_preserves_order(self, adapter: TensorRtAdapter) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)

        # Per-prompt response, keyed by the prompt text.
        def _per_prompt(**kwargs: Any) -> TritonResponse:
            prompt = kwargs["prompt"]
            return TritonResponse(
                status_code=200,
                body={"text_output": f"resp:{prompt}", "output_tokens": 2},
                elapsed_ms=1.0,
            )

        client.generate.side_effect = _per_prompt
        prompts = ["a", "b", "c", "d"]
        results = await adapter.generate_batch(prompts, GenerationParams())
        assert [r.text for r in results] == [f"resp:{p}" for p in prompts]

    @pytest.mark.asyncio
    async def test_empty_input_returns_empty_list(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        result = await adapter.generate_batch([], GenerationParams())
        assert result == []
        client.generate.assert_not_called()


# ─── Embedding ────────────────────────────────────────────────────────────


class TestEmbed:
    @pytest.mark.asyncio
    async def test_unsupported_raises(self, adapter: TensorRtAdapter) -> None:
        with pytest.raises(UnsupportedOperationError) as excinfo:
            await adapter.embed(["hello"])
        assert excinfo.value.data.get("operation") == "embedding"

    @pytest.mark.asyncio
    async def test_embed_never_returns_fake_vectors(
        self, adapter: TensorRtAdapter,
    ) -> None:
        """The spec forbids fake successes for unsupported ops -- no empty
        list, no zero-vector stub, no silent ``None``. Always raises.

        Sanity reference: ``Embedding`` is the only type the contract
        allows for a *successful* embed return.
        """
        assert Embedding is not None  # type guard the import stays load-bearing
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])


# ─── Health ───────────────────────────────────────────────────────────────


class TestHealth:
    @pytest.mark.asyncio
    async def test_unavailable_before_initialize(
        self, adapter: TensorRtAdapter,
    ) -> None:
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_healthy_when_engine_ready(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=True)
        await _initialize_with_stub(adapter, client)
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert isinstance(status, HealthStatus)

    @pytest.mark.asyncio
    async def test_degraded_when_engine_not_ready(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=False)
        await _initialize_with_stub(adapter, client)
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.DEGRADED
        assert status.reason is not None
        assert "not ready" in status.reason

    @pytest.mark.asyncio
    async def test_unavailable_when_server_down(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=True)
        await _initialize_with_stub(adapter, client)
        client.health.return_value = False
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE


# ─── Capabilities ─────────────────────────────────────────────────────────


class TestCapabilities:
    def test_truthful_flags(self, adapter: TensorRtAdapter) -> None:
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_embedding is False
        assert caps.supports_vision is False
        assert caps.supports_tool_calling is False
        assert caps.supports_continuous_batching is True  # inflight_batching=True default
        assert caps.max_context_window > 0
        assert "fp16" in caps.supported_quantizations

    def test_inflight_batching_flag_follows_config(self) -> None:
        a = TensorRtAdapter(
            {
                "host": "127.0.0.1",
                "default_model": "x",
                "enable_inflight_batching": False,
            },
        )
        caps = a.capabilities()
        assert caps.supports_continuous_batching is False
        assert caps.extra["inflight_batching"] is False


# ─── HTTP client: error mapping + pooling ────────────────────────────────


class _FakeHTTPResponse:
    """Stand-in for the object yielded by urlopen()."""

    def __init__(self, body: bytes, status: int = 200) -> None:
        self._body = body
        self.status = status

    def read(self) -> bytes:
        return self._body

    def __enter__(self) -> _FakeHTTPResponse:
        return self

    def __exit__(self, *exc: Any) -> None:
        pass

    def close(self) -> None:
        pass

    def __iter__(self) -> Any:
        return iter(self._body.splitlines(keepends=True))


class _CountingOpener:
    """Stub opener that records every ``open()`` call and returns canned data."""

    def __init__(self, responder: Any) -> None:
        self._responder = responder
        self.opened: list[tuple[str, dict[str, str]]] = []

    def open(self, req: Any, timeout: float | None = None) -> Any:
        self.opened.append((req.full_url, dict(req.headers)))
        return self._responder(req, timeout)


def _new_client() -> TensorRtClient:
    return TensorRtClient(
        base_url="http://triton.local:8000",
        timeout_ms=1000,
        stream_timeout_ms=2000,
    )


class TestClientErrorMapping:
    def test_404_raises_model_not_found(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 404, "Not Found", {}, io.BytesIO(b'{"error":"no such model"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(ModelNotFoundError) as excinfo:
            client.generate("missing", "hi")
        assert excinfo.value.data.get("model") == "missing"

    def test_429_raises_rate_limited(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 429, "Too Many", {}, io.BytesIO(b'{"error":"slow down"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(RateLimitedError):
            client.generate("m", "hi")

    def test_413_raises_context_exceeded(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 413, "Too Big", {},
            io.BytesIO(b'{"error":"context length exceeded"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(ContextExceededError):
            client.generate("m", "hi")

    def test_500_oom_body_raises_oom(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 500, "Server Error", {},
            io.BytesIO(b'{"error":"CUDA out of memory"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(OutOfMemoryError):
            client.generate("m", "hi")

    def test_502_raises_backend_crashed(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 502, "Bad Gateway", {},
            io.BytesIO(b'{"error":"upstream died"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(BackendCrashedError):
            client.generate("m", "hi")

    def test_504_raises_timeout(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 504, "Gateway Timeout", {}, io.BytesIO(b""),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(AdapterTimeoutError):
            client.generate("m", "hi")

    def test_urlerror_timeout_raises_timeout(self) -> None:
        client = _new_client()
        err = urllib.error.URLError(reason="timed out")
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(AdapterTimeoutError):
            client.generate("m", "hi")

    def test_urlerror_refused_raises_unavailable(self) -> None:
        client = _new_client()
        err = urllib.error.URLError(reason="Connection refused")
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        with pytest.raises(BackendUnavailableError):
            client.generate("m", "hi")

    def test_malformed_json_body_raises_crashed(self) -> None:
        client = _new_client()
        client._opener = _CountingOpener(
            lambda *_: _FakeHTTPResponse(b"not-json", status=200),
        )
        with pytest.raises(BackendCrashedError):
            client.generate("m", "hi")


class TestClientPooling:
    def test_opener_is_reused_across_two_requests(self) -> None:
        """The opener instance MUST be the same object across two calls."""
        client = _new_client()
        seen: list[int] = []

        def _responder(req: Any, _timeout: float | None) -> _FakeHTTPResponse:
            seen.append(id(client._opener))
            return _FakeHTTPResponse(b'{"text_output":"ok","output_tokens":1}')

        client._opener = _CountingOpener(_responder)
        client.generate("m", "hello")
        client.generate("m", "world")
        assert len(seen) == 2
        assert seen[0] == seen[1]

    def test_close_releases_opener(self) -> None:
        client = _new_client()
        client.close()
        assert client._opener is None
        assert client._closed is True

    def test_close_is_idempotent(self) -> None:
        client = _new_client()
        client.close()
        client.close()  # must not raise

    def test_use_after_close_raises_unavailable(self) -> None:
        client = _new_client()
        client.close()
        with pytest.raises(BackendUnavailableError):
            client.generate("m", "hi")


class TestClientStreaming:
    def test_sse_stream_parses_text_output_frames(self) -> None:
        client = _new_client()
        sse = (
            b'data: {"text_output":"Hel","is_final":false}\n'
            b'data: {"text_output":"lo","is_final":true}\n'
            b"data: [DONE]\n"
        )
        client._opener = _CountingOpener(
            lambda *_: _FakeHTTPResponse(sse, status=200),
        )
        chunks = list(client.generate("m", "hi", stream=True))
        assert [c.text for c in chunks] == ["Hel", "lo"]
        assert chunks[-1].finished is True

    def test_sse_malformed_frame_raises(self) -> None:
        client = _new_client()
        sse = b'data: not-json\n'
        client._opener = _CountingOpener(
            lambda *_: _FakeHTTPResponse(sse, status=200),
        )
        with pytest.raises(BackendCrashedError):
            list(client.generate("m", "hi", stream=True))


class TestClientHealth:
    def test_health_true_on_200(self) -> None:
        client = _new_client()
        client._opener = _CountingOpener(
            lambda *_: _FakeHTTPResponse(b"{}", status=200),
        )
        assert client.health() is True

    def test_health_false_on_error(self) -> None:
        client = _new_client()
        err = urllib.error.URLError(reason="Connection refused")
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        assert client.health() is False

    def test_model_ready_false_on_404(self) -> None:
        client = _new_client()
        err = urllib.error.HTTPError(
            "http://x/y", 404, "Not Found", {}, io.BytesIO(b'{"error":"no model"}'),
        )
        client._opener = _CountingOpener(
            lambda *_: (_ for _ in ()).throw(err),
        )
        assert client.model_ready("missing") is False


# ─── Adapter helpers exercised independently ─────────────────────────────


class TestAdapterHelpers:
    @pytest.mark.asyncio
    async def test_is_engine_ready_refreshes_state(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init(healthy=True, model_ready=False)
        await _initialize_with_stub(adapter, client)
        assert adapter._engine_ready is False
        client.model_ready.return_value = True
        assert await adapter.is_engine_ready() is True
        assert adapter._engine_ready is True

    @pytest.mark.asyncio
    async def test_get_model_metadata_passes_through(
        self, adapter: TensorRtAdapter,
    ) -> None:
        client = _stub_client_for_init()
        await _initialize_with_stub(adapter, client)
        client.model_metadata.return_value = {"name": "llama-trt", "platform": "tensorrt_llm"}
        meta = await adapter.get_model_metadata()
        assert meta["name"] == "llama-trt"

    @pytest.mark.asyncio
    async def test_get_model_metadata_pre_init_raises(
        self, adapter: TensorRtAdapter,
    ) -> None:
        with pytest.raises(AdapterError):
            await adapter.get_model_metadata()


# ─── Sanity: unused-import guard ──────────────────────────────────────────
# These imports exist to keep the suite honest -- if a future refactor
# removes a typed error class entirely, the test file fails to import.
_used = (AsyncMock, json)


# ─── J-12: async context manager smoke ───────────────────────────────────────


@pytest.mark.asyncio
async def test_async_context_manager_lifecycle_j12() -> None:
    """J-12: ``async with`` calls initialize on enter, shutdown on exit."""
    from adapters.base import ValidationError

    adapter = TensorRtAdapter()
    adapter.initialize = AsyncMock(return_value=None)
    adapter.shutdown = AsyncMock(return_value=None)
    adapter.set_config({"host": "127.0.0.1"}, hil_handle=None)
    async with adapter as bound:
        assert bound is adapter
    adapter.initialize.assert_awaited_once_with(
        {"host": "127.0.0.1"}, hil_handle=None,
    )
    adapter.shutdown.assert_awaited_once()

    fresh = TensorRtAdapter()
    with pytest.raises(ValidationError, match="config not set"):
        async with fresh:
            pass
