"""Unit tests for the MAI SGLang adapter.

J-20 (DOUGHERTY lane). These tests prove the adapter against the shared
adapter contract under `docs/ADAPTER-SHARED-CONTRACT.md` and the unit
minimums in `docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Unit Test Minimums".

The backend boundary is mocked at the `SglangClient` level. Live
backend coverage is in `test_integration_live.py`; real HTTP/SSE
wire coverage is in `test_integration_mock.py`.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any
from unittest.mock import MagicMock

import pytest

from adapters.base import (
    AdapterCapabilities,
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    GenerationParams,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    Token,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.sglang.adapter import SglangAdapter
from adapters.sglang.client import SglangClient, SglangResponse, SglangStreamChunk
from adapters.sglang.config import SglangConfig

# ─── helpers ────────────────────────────────────────────────────────────────


def _ok_response(text: str = "Hello!", completion_tokens: int = 2) -> SglangResponse:
    return SglangResponse(
        status_code=200,
        body={
            "choices": [{
                "message": {"content": text},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": completion_tokens},
        },
        elapsed_ms=1.0,
    )


def _stream(chunks: list[tuple[str, str | None]]) -> Iterator[SglangStreamChunk]:
    return iter(
        SglangStreamChunk(content=c, finish_reason=f) for c, f in chunks
    )


def _fresh(config: dict[str, Any] | None = None) -> SglangAdapter:
    cfg = config or {"host": "127.0.0.1", "port": 30000, "enable_radix_attention": True}
    return SglangAdapter(cfg)


def _bind_mock_client(adapter: SglangAdapter, **overrides: Any) -> MagicMock:
    """Inject a MagicMock client into the adapter so initialize() reuses
    it (rather than constructing a real `SglangClient`)."""
    client = MagicMock(spec=SglangClient)
    client.health.return_value = overrides.pop("health", True)
    client.models.return_value = overrides.pop("models", [{"id": "test-model"}])
    client.flush_cache.return_value = overrides.pop("flush_cache", True)
    client.get_model_info.return_value = overrides.pop("get_model_info", {"model_id": "test-model"})
    if "chat_completions" in overrides:
        client.chat_completions.return_value = overrides.pop("chat_completions")
    if "chat_completions_side_effect" in overrides:
        client.chat_completions.side_effect = overrides.pop("chat_completions_side_effect")
    if "generate" in overrides:
        client.generate.return_value = overrides.pop("generate")
    adapter._client = client
    return client


# ─── config ─────────────────────────────────────────────────────────────────


class TestSglangConfig:
    def test_defaults(self) -> None:
        cfg = SglangConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 30000
        assert cfg.enable_radix_attention is True
        assert cfg.timeout_ms == 30000
        assert cfg.stream_timeout_ms == 180000
        assert cfg.base_url == "http://127.0.0.1:30000"

    def test_custom_passthrough(self) -> None:
        cfg = SglangConfig.from_dict({"enable_vision": True, "max_forks": 16})
        assert cfg.enable_vision is True
        assert cfg.max_forks == 16

    def test_extra_options_collected(self) -> None:
        cfg = SglangConfig.from_dict({"host": "h", "unknown_key": "x"})
        assert cfg.host == "h"
        assert cfg.extra_options == {"unknown_key": "x"}


# ─── construction & initialize ──────────────────────────────────────────────


class TestConstruction:
    def test_construction_stores_config_without_network(self) -> None:
        """Contract: __init__ stores config only — no sockets, no clients."""
        adapter = _fresh()
        assert adapter._client is None
        assert adapter._cfg is None
        assert adapter._initialized is False
        assert adapter._requests_served == 0


class TestInitialize:
    @pytest.mark.asyncio
    async def test_initialize_happy_path(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        handle = await adapter.initialize()
        assert adapter._initialized is True
        assert handle == "test-model"
        assert adapter._model_id == "test-model"

    @pytest.mark.asyncio
    async def test_initialize_returns_handle_string(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter, models=[{"id": "meta-llama/Llama-3.1-8B"}])
        handle = await adapter.initialize()
        assert handle == "meta-llama/Llama-3.1-8B"

    @pytest.mark.asyncio
    async def test_initialize_prefers_configured_default_model(self) -> None:
        adapter = _fresh({"host": "127.0.0.1", "port": 30000, "default_model": "configured-model"})
        # models endpoint should NOT be consulted when default_model is set.
        client = _bind_mock_client(adapter, models=[{"id": "from-server"}])
        handle = await adapter.initialize()
        assert handle == "configured-model"
        client.models.assert_not_called()

    @pytest.mark.asyncio
    async def test_initialize_unavailable_backend_raises_typed(self) -> None:
        """Contract: a backend that does not respond healthy maps to
        BackendUnavailableError, not a raw exception."""
        adapter = _fresh()
        _bind_mock_client(adapter, health=False)
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_initialize_validation_error_for_bad_port(self) -> None:
        adapter = _fresh({"host": "127.0.0.1", "port": 99999})
        with pytest.raises(ValidationError):
            await adapter.initialize()

    @pytest.mark.asyncio
    async def test_initialize_validation_error_for_empty_host(self) -> None:
        adapter = _fresh({"host": "", "port": 30000})
        with pytest.raises(ValidationError):
            await adapter.initialize()

    @pytest.mark.asyncio
    async def test_initialize_propagates_health_oserror_as_typed(self) -> None:
        """Bare OSError from `health` becomes BackendUnavailableError."""
        adapter = _fresh()
        client = MagicMock(spec=SglangClient)
        client.health.side_effect = OSError("connection refused")
        adapter._client = client
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()


# ─── generate (non-streaming) ────────────────────────────────────────────────


class TestGenerateNonStreaming:
    @pytest.mark.asyncio
    async def test_generate_non_streaming_returns_generation_result(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter, chat_completions=_ok_response("Hi back", 3))
        await adapter.initialize()
        result = await adapter.generate("Hi", GenerationParams())
        assert result.text == "Hi back"
        assert result.tokens_generated == 3
        assert result.finish_reason.value == "stop"

    @pytest.mark.asyncio
    async def test_generate_accepts_raw_dict_response_for_test_compat(self) -> None:
        """Earlier tests mocked the client with raw dicts. The adapter
        accepts both `SglangResponse` and `dict` to keep the contract
        forgiving — but it never accepts None or other types."""
        adapter = _fresh()
        client = _bind_mock_client(
            adapter,
            chat_completions={
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
                "usage": {"completion_tokens": 1},
            },
        )
        await adapter.initialize()
        result = await adapter.generate("Hi", GenerationParams())
        assert result.text == "ok"
        client.chat_completions.assert_called_once()

    @pytest.mark.asyncio
    async def test_generate_constrained_json_schema_passed_through(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(
            adapter,
            chat_completions=_ok_response('{"name":"x"}', 5),
        )
        await adapter.initialize()
        params = GenerationParams(extra={"json_schema": {"type": "object"}})
        result = await adapter.generate("Generate JSON", params)
        assert "name" in result.text
        kwargs = client.chat_completions.call_args.kwargs
        assert kwargs["json_schema"] == {"type": "object"}

    @pytest.mark.asyncio
    async def test_generate_max_tokens_marks_finish_reason(self) -> None:
        adapter = _fresh()
        body = SglangResponse(
            status_code=200,
            body={
                "choices": [{"message": {"content": "..."}, "finish_reason": "length"}],
                "usage": {"completion_tokens": 100},
            },
            elapsed_ms=1.0,
        )
        _bind_mock_client(adapter, chat_completions=body)
        await adapter.initialize()
        result = await adapter.generate("anything", GenerationParams())
        assert result.finish_reason.value == "max_tokens"

    @pytest.mark.asyncio
    async def test_generate_timeout_maps_to_adapter_timeout(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions_side_effect=AdapterTimeoutError(timeout_ms=1000),
        )
        await adapter.initialize()
        with pytest.raises(AdapterTimeoutError):
            await adapter.generate("Hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_model_not_found_propagates_typed(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions_side_effect=ModelNotFoundError(model="ghost"),
        )
        await adapter.initialize()
        with pytest.raises(ModelNotFoundError):
            await adapter.generate("Hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_oom_propagates_typed(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions_side_effect=OutOfMemoryError(),
        )
        await adapter.initialize()
        with pytest.raises(OutOfMemoryError):
            await adapter.generate("Hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_malformed_response_raises_typed_error(self) -> None:
        adapter = _fresh()
        # Response missing required 'choices' key — adapter must raise
        # a typed adapter error rather than leak the KeyError.
        _bind_mock_client(
            adapter,
            chat_completions=SglangResponse(status_code=200, body={"usage": {}}, elapsed_ms=1.0),
        )
        await adapter.initialize()
        with pytest.raises(BackendCrashedError):
            await adapter.generate("Hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_oserror_maps_to_backend_crashed(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions_side_effect=OSError("conn reset"),
        )
        await adapter.initialize()
        with pytest.raises(BackendCrashedError):
            await adapter.generate("Hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_uninitialized_raises(self) -> None:
        adapter = _fresh()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("Hi", GenerationParams())


# ─── generate (streaming) ────────────────────────────────────────────────────


class TestGenerateStreaming:
    @pytest.mark.asyncio
    async def test_stream_yields_ordered_tokens(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions=_stream([("Hello", None), (" world", None), ("", "stop")]),
        )
        await adapter.initialize()
        tokens: list[Token] = []
        async for tok in await adapter.generate("Hi", GenerationParams(), stream=True):
            tokens.append(tok)
        # Three yields: "Hello", " world", and the end-marker.
        assert len(tokens) == 3
        assert [t.text for t in tokens] == ["Hello", " world", ""]
        assert [t.index for t in tokens] == [0, 1, 2]
        assert tokens[-1].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_stream_terminates_without_done_marker(self) -> None:
        """Even if the backend forgets the terminal chunk, iteration
        ends cleanly when the underlying iterator is exhausted."""
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions=_stream([("A", None), ("B", None)]),
        )
        await adapter.initialize()
        tokens = [t async for t in await adapter.generate("Hi", GenerationParams(), stream=True)]
        assert [t.text for t in tokens] == ["A", "B"]
        # No finish_reason ever arrived; is_end_of_text stays False.
        assert all(t.is_end_of_text is False for t in tokens)

    @pytest.mark.asyncio
    async def test_stream_skips_empty_mid_chunks(self) -> None:
        adapter = _fresh()
        _bind_mock_client(
            adapter,
            chat_completions=_stream([("", None), ("payload", None), ("", "stop")]),
        )
        await adapter.initialize()
        texts = [
            t.text
            async for t in await adapter.generate("Hi", GenerationParams(), stream=True)
        ]
        # First chunk is empty AND has no finish_reason → suppressed.
        # Last chunk is empty BUT has finish_reason → emitted as EOT.
        assert texts == ["payload", ""]


# ─── generate_batch ──────────────────────────────────────────────────────────


class TestGenerateBatch:
    @pytest.mark.asyncio
    async def test_batch_preserves_order(self) -> None:
        adapter = _fresh()
        responses = [
            _ok_response("one", 1),
            _ok_response("two", 1),
            _ok_response("three", 1),
        ]
        client = _bind_mock_client(adapter)
        client.chat_completions.side_effect = responses
        await adapter.initialize()
        # Re-set side_effect because initialize() consumed nothing from it,
        # but a defensive reset keeps the intent explicit.
        client.chat_completions.side_effect = responses
        out = await adapter.generate_batch(["a", "b", "c"], GenerationParams())
        assert [r.text for r in out] == ["one", "two", "three"]

    @pytest.mark.asyncio
    async def test_batch_empty_input_returns_empty_list(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        out = await adapter.generate_batch([], GenerationParams())
        assert out == []

    @pytest.mark.asyncio
    async def test_batch_uninitialized_raises(self) -> None:
        adapter = _fresh()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate_batch(["a"], GenerationParams())


# ─── embed ───────────────────────────────────────────────────────────────────


class TestEmbed:
    @pytest.mark.asyncio
    async def test_embed_raises_unsupported(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        with pytest.raises(UnsupportedOperationError) as ei:
            await adapter.embed(["hello"])
        assert ei.value.data.get("operation") == "embed"

    @pytest.mark.asyncio
    async def test_embed_uninitialized_raises_backend_unavailable(self) -> None:
        adapter = _fresh()
        with pytest.raises(BackendUnavailableError):
            await adapter.embed(["hello"])


# ─── health ──────────────────────────────────────────────────────────────────


class TestHealth:
    @pytest.mark.asyncio
    async def test_health_unavailable_before_init(self) -> None:
        adapter = _fresh()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert status.healthy is False

    @pytest.mark.asyncio
    async def test_health_healthy_after_init(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.healthy is True
        assert status.uptime_ms >= 0

    @pytest.mark.asyncio
    async def test_health_degraded_when_backend_drops(self) -> None:
        """After init, if a later health probe reports False, the
        adapter must report DEGRADED — not UNAVAILABLE — because the
        client itself is reachable."""
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        await adapter.initialize()
        # Now flip the backend's health response.
        client.health.return_value = False
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.DEGRADED
        assert status.reason is not None and "did not return ok" in status.reason

    @pytest.mark.asyncio
    async def test_health_unavailable_when_health_raises_oserror(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        await adapter.initialize()
        client.health.side_effect = OSError("conn refused")
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_health_counts_served_requests(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter, chat_completions=_ok_response("a", 1))
        await adapter.initialize()
        await adapter.generate("hi", GenerationParams())
        status = await adapter.health_check()
        assert status.requests_served == 1


# ─── capabilities ────────────────────────────────────────────────────────────


class TestCapabilities:
    def test_capabilities_truthful_for_each_flag(self) -> None:
        adapter = _fresh()
        adapter._cfg = SglangConfig.from_dict({})
        caps = adapter.capabilities()
        assert isinstance(caps, AdapterCapabilities)
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_embedding is False
        assert caps.supports_embeddings is False  # alias
        assert caps.supports_tool_calling is True
        assert caps.supports_structured_output is True
        assert caps.max_context_window == 131072

    def test_capabilities_reflect_radix_config(self) -> None:
        adapter = _fresh({"enable_radix_attention": False})
        adapter._cfg = SglangConfig.from_dict({"enable_radix_attention": False})
        caps = adapter.capabilities()
        assert caps.extra["radix_attention"] is False

    def test_capabilities_reflect_vision_config(self) -> None:
        adapter = _fresh({"enable_vision": True})
        adapter._cfg = SglangConfig.from_dict({"enable_vision": True})
        caps = adapter.capabilities()
        assert caps.extra["vision"] is True

    def test_capabilities_quantizations_documented(self) -> None:
        adapter = _fresh()
        adapter._cfg = SglangConfig.from_dict({})
        caps = adapter.capabilities()
        for q in ("fp16", "fp8", "awq", "gptq"):
            assert q in caps.supported_quantizations


# ─── pooling / lifecycle ─────────────────────────────────────────────────────


class TestPoolingAndLifecycle:
    @pytest.mark.asyncio
    async def test_client_reused_across_two_calls(self) -> None:
        """Contract: HTTP client object is reused across requests."""
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        # Provide two distinct successful responses to two generates.
        client.chat_completions.side_effect = [_ok_response("a"), _ok_response("b")]
        await adapter.initialize()
        first = adapter._client
        await adapter.generate("a", GenerationParams())
        await adapter.generate("b", GenerationParams())
        # Same object reference both times — no per-request reconstruction.
        assert adapter._client is first
        assert client.chat_completions.call_count == 2

    @pytest.mark.asyncio
    async def test_shutdown_releases_client(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None
        assert adapter._model_id is None

    @pytest.mark.asyncio
    async def test_shutdown_idempotent(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        await adapter.shutdown()
        await adapter.shutdown()  # must not raise
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_post_shutdown_generate_fails_deterministically(self) -> None:
        adapter = _fresh()
        _bind_mock_client(adapter)
        await adapter.initialize()
        await adapter.shutdown()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams())


# ─── native SGLang surface ───────────────────────────────────────────────────


class TestNativeSurface:
    @pytest.mark.asyncio
    async def test_flush_cache_passes_through(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        await adapter.initialize()
        assert await adapter.flush_cache() is True
        client.flush_cache.assert_called_once()

    @pytest.mark.asyncio
    async def test_get_model_info_passes_through(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(adapter, get_model_info={"model_id": "x"})
        await adapter.initialize()
        info = await adapter.get_model_info()
        assert info == {"model_id": "x"}
        client.get_model_info.assert_called_once()

    @pytest.mark.asyncio
    async def test_generate_native_returns_generation_result(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        client.generate.return_value = SglangResponse(
            status_code=200,
            body={
                "text": "constrained",
                "meta_info": {"finish_reason": "stop", "completion_tokens": 2},
            },
            elapsed_ms=1.0,
        )
        await adapter.initialize()
        result = await adapter.generate_native(
            "prompt",
            GenerationParams(max_tokens=10),
            json_schema={"type": "object"},
        )
        assert result.text == "constrained"
        assert result.tokens_generated == 2
        kwargs = client.generate.call_args.kwargs
        assert kwargs["json_schema"] == {"type": "object"}

    @pytest.mark.asyncio
    async def test_generate_native_regex_passed_through(self) -> None:
        adapter = _fresh()
        client = _bind_mock_client(adapter)
        client.generate.return_value = SglangResponse(
            status_code=200,
            body={"text": "yes", "meta_info": {"finish_reason": "stop"}},
            elapsed_ms=1.0,
        )
        await adapter.initialize()
        await adapter.generate_native("Q?", GenerationParams(), regex="(yes|no)")
        kwargs = client.generate.call_args.kwargs
        assert kwargs["regex"] == "(yes|no)"
