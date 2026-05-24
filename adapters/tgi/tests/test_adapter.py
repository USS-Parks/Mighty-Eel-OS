"""Unit tests for the MAI TGI adapter.

DOUGHERTY J-19. Closes the unit-test gaps from
`docs/ADAPTER-SHARED-CONTRACT.md` (required surface) and
`docs/ADAPTER-TEST-HARNESS-LOCK.md` (unit-test minimums) for the
HuggingFace Text Generation Inference backend.

No live TGI server is required. Live tests live in
`test_integration_live.py` (opt-in, gated by ``TGI_HOST``). HTTP mock
tests live in `test_integration_mock.py`.
"""
from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    ContextExceededError,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    Token,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.tgi.adapter import TgiAdapter
from adapters.tgi.client import TgiResponse, TgiStreamChunk
from adapters.tgi.config import TgiConfig


def _make_client(*, health: bool = True, info: dict[str, Any] | None = None) -> MagicMock:
    """Build a MagicMock TgiClient with sync-callable probes.

    The adapter calls these via ``maybe_await``, which is tolerant of both
    sync and async return values, so plain MagicMock is enough.
    """
    client = MagicMock()
    client.health = MagicMock(return_value=health)
    client.info = MagicMock(return_value=info or {"model_id": "test-model"})
    return client


@pytest.fixture
def config():
    return {"host": "127.0.0.1", "port": 8080}


@pytest.fixture
def adapter(config):
    return TgiAdapter(config)


class TestTgiConfig:
    def test_defaults(self):
        cfg = TgiConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8080
        assert cfg.quantize is None
        assert cfg.base_url == "http://127.0.0.1:8080"

    def test_custom(self):
        cfg = TgiConfig.from_dict({"quantize": "bitsandbytes-nf4", "speculate": 3})
        assert cfg.quantize == "bitsandbytes-nf4"
        assert cfg.speculate == 3

    def test_from_dict_routes_unknown_keys_to_extra(self):
        cfg = TgiConfig.from_dict({"host": "10.0.0.1", "weird_opt": 42})
        assert cfg.host == "10.0.0.1"
        assert cfg.extra_options == {"weird_opt": 42}

    def test_base_url_uses_port(self):
        cfg = TgiConfig.from_dict({"host": "tgi.local", "port": 9000})
        assert cfg.base_url == "http://tgi.local:9000"


class TestTgiAdapterConstruction:
    def test_init_stores_config_without_network(self, config):
        a = TgiAdapter(config)
        assert a._initialized is False
        assert a._client is None
        assert a._requests_served == 0
        assert a._start_time_ms == 0

    def test_init_with_no_config(self):
        a = TgiAdapter()
        assert a._initialized is False
        assert a._client is None


class TestTgiAdapterInitialize:
    @pytest.mark.asyncio
    async def test_happy_path_sets_state_from_info(self, adapter):
        adapter._client = _make_client(info={
            "model_id": "mistralai/Mistral-7B",
            "max_input_length": 2048,
            "max_total_tokens": 4096,
        })
        handle = await adapter.initialize()
        assert adapter._initialized is True
        assert adapter._model_id == "mistralai/Mistral-7B"
        assert adapter._max_input_tokens == 2048
        assert adapter._max_total_tokens == 4096
        assert handle.startswith("tgi-mistralai/Mistral-7B-")

    @pytest.mark.asyncio
    async def test_legacy_cfg_path_still_supported(self, adapter):
        # Pre-existing tests injected config via the `_cfg` attribute;
        # keep that backward-compat shim alive.
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.info = AsyncMock(return_value={"model_id": "x"})
        adapter._cfg = TgiConfig.from_dict({})
        await adapter.initialize()
        assert adapter._initialized is True
        assert adapter._model_id == "x"

    @pytest.mark.asyncio
    async def test_unavailable_backend_maps_to_typed_error(self, adapter):
        adapter._client = _make_client(health=False)
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_validation_error_on_empty_host(self):
        a = TgiAdapter()
        with pytest.raises(ValidationError):
            await a.initialize(config={"host": "", "port": 8080})

    @pytest.mark.asyncio
    async def test_validation_error_on_bad_timeout(self):
        a = TgiAdapter()
        with pytest.raises(ValidationError):
            await a.initialize(config={"timeout_ms": 0})

    @pytest.mark.asyncio
    async def test_reinitialize_without_new_config_reuses_client(self, adapter):
        client = _make_client()
        adapter._client = client
        await adapter.initialize()
        await adapter.initialize()
        assert adapter._client is client

    @pytest.mark.asyncio
    async def test_reinitialize_with_new_config_replaces_client(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        first = adapter._client
        replacement = _make_client(info={"model_id": "rebuilt"})
        with patch("adapters.tgi.adapter.TgiClient", return_value=replacement):
            await adapter.initialize(
                config={"host": "10.0.0.2", "port": 8080},
            )
        assert adapter._client is replacement
        assert adapter._client is not first
        assert adapter._model_id == "rebuilt"


class TestTgiAdapterPreInit:
    @pytest.mark.asyncio
    async def test_generate_before_initialize_raises(self):
        a = TgiAdapter()
        with pytest.raises(BackendUnavailableError):
            await a.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_batch_before_initialize_raises(self):
        a = TgiAdapter()
        with pytest.raises(BackendUnavailableError):
            await a.generate_batch(["hi"], GenerationParams())

    @pytest.mark.asyncio
    async def test_health_before_initialize_returns_unavailable(self):
        a = TgiAdapter()
        status = await a.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert status.healthy is False


class TestTgiAdapterGenerate:
    @pytest.mark.asyncio
    async def test_happy_path(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value={
            "generated_text": "Hello world",
            "details": {"generated_tokens": 3, "finish_reason": "length"},
        })
        result = await adapter.generate("Hi", GenerationParams(max_tokens=10))
        assert isinstance(result, GenerationResult)
        assert result.text == "Hello world"
        assert result.tokens_generated == 3
        assert result.finish_reason == FinishReason.MAX_TOKENS
        assert adapter._requests_served == 1

    @pytest.mark.asyncio
    async def test_finish_reason_stop(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value={
            "generated_text": "done",
            "details": {"generated_tokens": 1, "finish_reason": "eos_token"},
        })
        result = await adapter.generate("hi", GenerationParams())
        assert result.finish_reason == FinishReason.STOP

    @pytest.mark.asyncio
    async def test_accepts_tgi_response_object(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value=TgiResponse(
            status_code=200,
            body={"generated_text": "ok",
                  "details": {"generated_tokens": 2, "finish_reason": "length"}},
            elapsed_ms=12.5,
        ))
        result = await adapter.generate("hi", GenerationParams())
        assert result.text == "ok"
        assert result.tokens_generated == 2

    @pytest.mark.asyncio
    async def test_missing_details_defaults_to_stop(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value={"generated_text": "ok"})
        result = await adapter.generate("hi", GenerationParams())
        assert result.finish_reason == FinishReason.STOP
        assert result.text == "ok"

    @pytest.mark.asyncio
    async def test_malformed_response_does_not_crash(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value="not a dict")
        result = await adapter.generate("hi", GenerationParams())
        assert result.text == ""
        assert result.tokens_generated == 0

    @pytest.mark.asyncio
    async def test_timeout_propagates(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(
            side_effect=AdapterTimeoutError(timeout_ms=30000),
        )
        with pytest.raises(AdapterTimeoutError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_model_not_found_propagates(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(
            side_effect=ModelNotFoundError("mistral-9000"),
        )
        with pytest.raises(ModelNotFoundError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_out_of_memory_propagates(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(side_effect=OutOfMemoryError())
        with pytest.raises(OutOfMemoryError):
            await adapter.generate("hi", GenerationParams())


class TestTgiAdapterGenerateStream:
    @pytest.mark.asyncio
    async def test_stream_yields_ordered_tokens(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value=iter([
            TgiStreamChunk("Hello"),
            TgiStreamChunk(" world"),
            TgiStreamChunk("!", finish_reason="length", generated_text="Hello world!"),
        ]))
        stream = await adapter.generate("hi", GenerationParams(), stream=True)
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)
        assert [t.text for t in tokens] == ["Hello", " world", "!"]
        assert [t.index for t in tokens] == [0, 1, 2]
        assert tokens[-1].is_end_of_text is True
        assert tokens[0].is_end_of_text is False

    @pytest.mark.asyncio
    async def test_stream_terminates_on_end_marker(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value=iter([
            TgiStreamChunk("hi", finish_reason="eos_token", generated_text="hi"),
        ]))
        stream = await adapter.generate("hi", GenerationParams(), stream=True)
        tokens: list[Token] = []
        async for tok in stream:
            tokens.append(tok)
        assert len(tokens) == 1
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_stream_propagates_context_exceeded(self, adapter):
        adapter._initialized = True

        def _raising_iter():
            yield TgiStreamChunk("he")
            raise ContextExceededError(max_context=4096)

        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(return_value=_raising_iter())
        stream = await adapter.generate("hi", GenerationParams(), stream=True)
        with pytest.raises(ContextExceededError):
            async for _ in stream:
                pass


class TestTgiAdapterBatch:
    @pytest.mark.asyncio
    async def test_preserves_order(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        responses = [
            {"generated_text": "alpha",
             "details": {"generated_tokens": 1, "finish_reason": "length"}},
            {"generated_text": "beta",
             "details": {"generated_tokens": 1, "finish_reason": "length"}},
            {"generated_text": "gamma",
             "details": {"generated_tokens": 1, "finish_reason": "length"}},
        ]
        adapter._client.generate = MagicMock(side_effect=responses)
        results = await adapter.generate_batch(
            ["a", "b", "c"], GenerationParams(max_tokens=8),
        )
        assert [r.text for r in results] == ["alpha", "beta", "gamma"]
        assert adapter._requests_served == 3

    @pytest.mark.asyncio
    async def test_empty_input_returns_empty(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        results = await adapter.generate_batch([], GenerationParams())
        assert results == []
        assert adapter._requests_served == 0

    @pytest.mark.asyncio
    async def test_per_prompt_error_propagates(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._client.generate = MagicMock(side_effect=[
            {"generated_text": "ok",
             "details": {"generated_tokens": 1, "finish_reason": "length"}},
            OutOfMemoryError(),
        ])
        with pytest.raises(OutOfMemoryError):
            await adapter.generate_batch(["a", "b"], GenerationParams())


class TestTgiAdapterEmbed:
    @pytest.mark.asyncio
    async def test_embed_raises(self, adapter):
        adapter._initialized = True
        with pytest.raises(UnsupportedOperationError) as ei:
            await adapter.embed(["hello"])
        assert "embed" in str(ei.value)

    @pytest.mark.asyncio
    async def test_embed_before_initialize_still_raises_unsupported(self):
        # TGI never supports embeddings; the contract permits raising
        # UnsupportedOperationError regardless of lifecycle state.
        a = TgiAdapter()
        with pytest.raises(UnsupportedOperationError):
            await a.embed(["hello"])


class TestTgiAdapterHealth:
    @pytest.mark.asyncio
    async def test_unavailable_when_not_initialized(self):
        a = TgiAdapter()
        status = await a.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_healthy_after_init(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.healthy is True
        assert status.uptime_ms >= 0

    @pytest.mark.asyncio
    async def test_degraded_when_health_fails_but_info_responds(self, adapter):
        client = MagicMock()
        client.health = MagicMock(side_effect=[True, False])
        client.info = MagicMock(return_value={"model_id": "mistral"})
        adapter._client = client
        await adapter.initialize()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.DEGRADED
        assert "health" in (status.reason or "").lower()

    @pytest.mark.asyncio
    async def test_unavailable_when_both_probes_fail(self, adapter):
        client = MagicMock()
        client.health = MagicMock(side_effect=[True, False])
        client.info = MagicMock(side_effect=[{"model_id": "mistral"}, {}])
        adapter._client = client
        await adapter.initialize()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE


class TestTgiAdapterCapabilities:
    def test_truthful_for_streaming(self, adapter):
        caps = adapter.capabilities()
        assert caps.supports_streaming is True

    def test_embedding_is_false(self, adapter):
        caps = adapter.capabilities()
        assert caps.supports_embedding is False
        assert caps.supports_embeddings is False

    def test_structured_output_is_false(self, adapter):
        # TGI does not implement constrained decoding in this adapter.
        caps = adapter.capabilities()
        assert caps.supports_structured_output is False

    def test_quantization_list_includes_known_kinds(self, adapter):
        caps = adapter.capabilities()
        for kind in ("bitsandbytes", "gptq", "awq"):
            assert kind in caps.supported_quantizations

    def test_continuous_batching_flag_matches_implementation(self, adapter):
        caps = adapter.capabilities()
        assert caps.supports_continuous_batching is True


class TestTgiAdapterShutdown:
    @pytest.mark.asyncio
    async def test_shutdown_clears_state(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None
        assert adapter._model_id == ""
        assert adapter._requests_served == 0

    @pytest.mark.asyncio
    async def test_double_shutdown_is_no_op(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        await adapter.shutdown()
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_shutdown_before_initialize_is_safe(self):
        a = TgiAdapter()
        await a.shutdown()
        assert a._initialized is False

    @pytest.mark.asyncio
    async def test_post_shutdown_generate_raises(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        await adapter.shutdown()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_post_shutdown_health_unavailable(self, adapter):
        adapter._client = _make_client()
        await adapter.initialize()
        await adapter.shutdown()
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE


class TestTgiAdapterSessionReuse:
    @pytest.mark.asyncio
    async def test_two_calls_share_client_object(self, adapter):
        client = MagicMock()
        client.health = MagicMock(return_value=True)
        client.info = MagicMock(return_value={"model_id": "x"})
        client.generate = MagicMock(return_value={
            "generated_text": "ok",
            "details": {"generated_tokens": 1, "finish_reason": "length"},
        })
        adapter._client = client
        await adapter.initialize()
        before = adapter._client
        await adapter.generate("a", GenerationParams())
        await adapter.generate("b", GenerationParams())
        assert adapter._client is before
        assert client.generate.call_count == 2
