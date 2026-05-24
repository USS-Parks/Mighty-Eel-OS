"""Unit tests for MAI ExLlamaV2 adapter.

J-09 (DOUGHERTY lane) grew the assertion floor from 12 to 30+ to clear
GitDoctor TST-004. Live-backend coverage is gated by EXLLAMAV2_HOST in
the live integration suite (J-21); this file stays mock-based.
"""
from __future__ import annotations

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
from adapters.exllamav2.config import ExLlamaV2Config


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
