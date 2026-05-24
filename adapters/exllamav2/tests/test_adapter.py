"""Unit tests for MAI ExLlamaV2 adapter.

J-09 (DOUGHERTY lane) grew the assertion floor from 12 to 30+ to clear
GitDoctor TST-004. The streaming path is exercised end-to-end against
a real localhost HTTP/SSE server (see
`adapters/tests/_streaming_server.py`) so the client's `urllib` call,
the `for line in resp:` SSE parser, and the adapter's
`_generate_stream` loop are all driven by real bytes — not by
`AsyncMock`. Live-backend coverage against a real TabbyAPI / ExLlamaV2
server is gated by `EXLLAMAV2_HOST` in the live integration suite
(J-21); that is a different, opt-in concern.
"""
from __future__ import annotations

import json
from unittest.mock import AsyncMock, MagicMock

import pytest

from adapters.base import (
    BackendUnavailableError,
    FinishReason,
    GenerationParams,
    HealthStatusKind,
    UnsupportedOperationError,
)
from adapters.exllamav2.adapter import ExLlamaV2Adapter
from adapters.exllamav2.client import ExLlamaV2Client
from adapters.exllamav2.config import ExLlamaV2Config
from adapters.tests._streaming_server import StreamRecipe, streaming_server


@pytest.fixture
def config():
    return {"host": "127.0.0.1", "port": 5000, "quantization": "exl2"}


@pytest.fixture
def adapter(config):
    return ExLlamaV2Adapter(config)


class TestExLlamaV2Config:
    def test_defaults(self):
        cfg = ExLlamaV2Config.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 5000
        assert cfg.quantization == "exl2"
        assert cfg.cache_mode == "Q4"

    def test_custom(self):
        cfg = ExLlamaV2Config.from_dict({"cache_mode": "FP16", "max_loaded_models": 3})
        assert cfg.cache_mode == "FP16"
        assert cfg.max_loaded_models == 3


class TestExLlamaV2Adapter:
    @pytest.mark.asyncio
    async def test_initialize(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.models = AsyncMock(return_value={
            "data": [{"id": "TheBloke/Llama-2-70B-EXL2"}]
        })
        adapter._cfg = ExLlamaV2Config.from_dict({})
        handle = await adapter.initialize()
        assert adapter._initialized is True
        assert isinstance(handle, str)
        assert handle.startswith("exllamav2-")
        assert "TheBloke/Llama-2-70B-EXL2" in adapter._loaded_models
        assert adapter._model == "TheBloke/Llama-2-70B-EXL2"

    @pytest.mark.asyncio
    async def test_initialize_backend_unavailable(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=False)
        adapter._cfg = ExLlamaV2Config.from_dict({})
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False
        assert adapter._loaded_models == []

    @pytest.mark.asyncio
    async def test_generate(self, adapter):
        adapter._initialized = True
        adapter._cfg = ExLlamaV2Config.from_dict({})
        adapter._client = AsyncMock()
        adapter._model_id = "test-model"
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "Answer"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 8, "completion_tokens": 3},
        })
        before = adapter._requests_served
        result = await adapter.generate("Question", GenerationParams())
        assert result.text == "Answer"
        assert result.tokens_generated == 3
        assert result.finish_reason == FinishReason.STOP
        assert adapter._requests_served == before + 1

    @pytest.mark.asyncio
    async def test_generate_max_tokens_finish(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "Cut"}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 1},
        })
        result = await adapter.generate("Q", GenerationParams())
        assert result.finish_reason == FinishReason.MAX_TOKENS
        assert result.tokens_generated == 1
        assert result.text == "Cut"

    @pytest.mark.asyncio
    async def test_generate_empty_choices(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.chat_completions = AsyncMock(return_value={"choices": []})
        result = await adapter.generate("Q", GenerationParams())
        assert result.text == ""
        assert result.tokens_generated == 0
        assert result.finish_reason == FinishReason.STOP

    @pytest.mark.asyncio
    async def test_generate_when_uninitialized_raises(self, adapter):
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("Q", GenerationParams())
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_embed_raises(self, adapter):
        adapter._initialized = True
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])

    def test_capabilities(self, adapter):
        adapter._cfg = ExLlamaV2Config.from_dict({})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_embedding is False
        assert caps.supports_batching is True
        assert caps.supports_hot_swap is True
        assert caps.supports_structured_output is False
        assert caps.supports_vision is False
        assert caps.supports_tool_calling is False
        assert caps.extra["multi_model"] is True
        assert "exl2" in caps.supported_quantizations
        assert "gptq" in caps.supported_quantizations

    @pytest.mark.asyncio
    async def test_health_check_healthy(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.healthy is True
        assert status.uptime_ms >= 0

    @pytest.mark.asyncio
    async def test_health_check_unavailable_when_uninitialized(self, adapter):
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert bool(status.healthy) is False

    @pytest.mark.asyncio
    async def test_health_check_unavailable_when_backend_down(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=False)
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert bool(status.healthy) is False

    @pytest.mark.asyncio
    async def test_shutdown_idempotent(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_load_model(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.model_load = AsyncMock(return_value=True)
        result = await adapter.load_model("new-model", {})
        assert result is True
        assert "new-model" in adapter._loaded_models
        assert adapter._model == "new-model"

    @pytest.mark.asyncio
    async def test_load_model_failure_returns_false(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.model_load = AsyncMock(side_effect=RuntimeError("OOM"))
        result = await adapter.load_model("too-big", {})
        assert result is False
        assert "too-big" not in adapter._loaded_models

    @pytest.mark.asyncio
    async def test_switch_model_known(self, adapter):
        adapter._initialized = True
        adapter._loaded_models = ["a", "b"]
        adapter._model = "a"
        ok = await adapter.switch_model("b")
        assert ok is True
        assert adapter._model == "b"

    @pytest.mark.asyncio
    async def test_switch_model_unknown_returns_false(self, adapter):
        adapter._initialized = True
        adapter._loaded_models = ["a"]
        adapter._model = "a"
        ok = await adapter.switch_model("nope")
        assert ok is False
        assert adapter._model == "a"

    @pytest.mark.asyncio
    async def test_unload_model_clears_active(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.model_unload = AsyncMock(return_value=True)
        adapter._loaded_models = ["a", "b"]
        adapter._model = "a"
        ok = await adapter.unload_model()
        assert ok is True
        assert "a" not in adapter._loaded_models
        assert adapter._model == "b"

    @pytest.mark.asyncio
    async def test_generate_batch(self, adapter):
        adapter._initialized = True
        adapter._cfg = ExLlamaV2Config.from_dict({})
        adapter._client = MagicMock()
        resp = MagicMock()
        resp.body = {
            "choices": [{"message": {"content": "A"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens": 1},
        }
        adapter._client.chat_completions = MagicMock(return_value=resp)
        before = adapter._requests_served
        results = await adapter.generate_batch(["Q1", "Q2"], GenerationParams())
        assert len(results) == 2
        assert results[0].text == "A"
        assert results[1].tokens_generated == 1
        assert adapter._requests_served == before + 2


class TestExLlamaV2Streaming:
    """Real-HTTP streaming tests against an actual SSE server.

    Each test stands up a `ThreadingHTTPServer` on a free localhost
    port, wires the adapter to a real `ExLlamaV2Client`, and drives
    `_generate_stream` through `asyncio.to_thread` + the real SSE
    parser. The chunk type here is `ExllamaStreamChunk(content,
    finish_reason)` (no `stop` field — the adapter derives
    is_end_of_text from `finish_reason is not None`). Live-backend
    coverage against a real TabbyAPI install is J-21 territory.
    """

    @pytest.fixture
    def adapter(self):
        a = ExLlamaV2Adapter()
        a._initialized = True
        a._config = ExLlamaV2Config.from_dict({})
        a._model = "test-model"
        return a

    @staticmethod
    def _wire(adapter: ExLlamaV2Adapter, url: str) -> None:
        adapter._client = ExLlamaV2Client(
            url, timeout_ms=2000, stream_timeout_ms=5000,
        )

    @pytest.mark.asyncio
    async def test_yields_tokens_in_order(self, adapter):
        recipe = StreamRecipe(chunks=[
            ("Hello", None),
            (" world", None),
            ("!", "stop"),
        ])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert [t.text for t in tokens] == ["Hello", " world", "!"]
        assert [t.index for t in tokens] == [0, 1, 2]
        assert tokens[-1].is_end_of_text is True
        assert tokens[0].is_end_of_text is False
        assert adapter._requests_served == 1

    @pytest.mark.asyncio
    async def test_synthetic_end_token_on_empty_final_chunk(self, adapter):
        recipe = StreamRecipe(chunks=[
            ("alpha", None),
            ("beta", None),
            ("", "stop"),
        ])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert [t.text for t in tokens] == ["alpha", "beta", ""]
        assert tokens[-1].is_end_of_text is True
        assert tokens[-1].text == ""
        assert tokens[1].is_end_of_text is False
        assert tokens[0].is_end_of_text is False

    @pytest.mark.asyncio
    async def test_ignores_malformed_data_lines(self, adapter):
        recipe = StreamRecipe(raw_data_payloads=[
            json.dumps({"choices": [{"delta": {"content": "first"}, "finish_reason": None}]}),
            "this is not json",
            json.dumps({"choices": [{"delta": {"content": "second"}, "finish_reason": "stop"}]}),
        ])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert [t.text for t in tokens] == ["first", "second"]
        assert len(tokens) == 2
        assert tokens[-1].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_ignores_chunks_with_no_choices(self, adapter):
        recipe = StreamRecipe(raw_data_payloads=[
            json.dumps({"choices": []}),
            json.dumps({"choices": [{"delta": {"content": "real"}, "finish_reason": "stop"}]}),
        ])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert len(tokens) == 1
        assert tokens[0].text == "real"
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_done_terminator_ends_stream(self, adapter):
        recipe = StreamRecipe(chunks=[("only", "stop")], include_done=True)
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert len(tokens) == 1
        assert tokens[0].text == "only"
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_max_tokens_finish_reason_marks_end(self, adapter):
        recipe = StreamRecipe(chunks=[("trunc", "length")])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        assert len(tokens) == 1
        assert tokens[0].text == "trunc"
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_request_counter_increments_after_full_drain(self, adapter):
        before = adapter._requests_served
        recipe = StreamRecipe(chunks=[("ok", "stop")])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            assert adapter._requests_served == before
            _ = [t async for t in gen]
        assert adapter._requests_served == before + 1
