"""Unit tests for MAI llama.cpp adapter.

J-09 (DOUGHERTY lane) grew the assertion floor from 13 to 30+ to clear
GitDoctor TST-004. Live-backend coverage lives in test_integration_live.py
(J-07); this file stays mock-based.
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
from adapters.llamacpp.adapter import LlamaCppAdapter
from adapters.llamacpp.config import LlamaCppConfig


@pytest.fixture
def config():
    return {"host": "127.0.0.1", "port": 8080, "n_gpu_layers": 35}


@pytest.fixture
def adapter(config):
    return LlamaCppAdapter(config)


class TestLlamaCppConfig:
    def test_defaults(self):
        cfg = LlamaCppConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8080
        assert cfg.n_gpu_layers == -1
        assert cfg.context_size == 8192

    def test_custom(self):
        cfg = LlamaCppConfig.from_dict({"n_gpu_layers": 40, "use_mlock": True})
        assert cfg.n_gpu_layers == 40
        assert cfg.use_mlock is True


class TestLlamaCppAdapter:
    @pytest.mark.asyncio
    async def test_initialize(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value={"status": "ok"})
        adapter._client.props = AsyncMock(return_value={
            "default_generation_settings": {"model": "test.gguf"}
        })
        adapter._cfg = LlamaCppConfig.from_dict({})
        handle = await adapter.initialize()
        assert adapter._initialized is True
        assert isinstance(handle, str)
        assert handle.startswith("llamacpp-")
        assert adapter._start_time_ms > 0

    @pytest.mark.asyncio
    async def test_initialize_backend_unavailable(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value={"status": "error"})
        adapter._cfg = LlamaCppConfig.from_dict({})
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False
        assert adapter._requests_served == 0

    @pytest.mark.asyncio
    async def test_generate(self, adapter):
        adapter._initialized = True
        adapter._cfg = LlamaCppConfig.from_dict({})
        adapter._client = AsyncMock()
        adapter._model_id = "test.gguf"
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "Response"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
        })
        before = adapter._requests_served
        result = await adapter.generate("Hi", GenerationParams())
        assert result.text == "Response"
        assert result.tokens_generated == 5
        assert result.finish_reason == FinishReason.STOP
        assert adapter._requests_served == before + 1

    @pytest.mark.asyncio
    async def test_generate_max_tokens_finish(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "Truncated"}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 2},
        })
        result = await adapter.generate("Q", GenerationParams())
        assert result.finish_reason == FinishReason.MAX_TOKENS
        assert result.tokens_generated == 2
        assert result.text == "Truncated"

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
        adapter._cfg = LlamaCppConfig.from_dict({})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_embedding is False
        assert caps.supports_structured_output is True
        assert caps.supports_batching is False
        assert caps.supports_continuous_batching is False
        assert caps.supports_vision is False
        assert caps.supports_tool_calling is False
        assert caps.supports_hot_swap is False
        assert "gguf_q4_K_M" in caps.supported_quantizations
        assert caps.backend_version.startswith("b")

    @pytest.mark.asyncio
    async def test_health_check_healthy(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model_id = "test.gguf"
        adapter._client.health = AsyncMock(return_value={"status": "ok"})
        status = await adapter.health_check()
        assert status.healthy is True
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0

    @pytest.mark.asyncio
    async def test_health_check_unavailable_when_uninitialized(self, adapter):
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert bool(status.healthy) is False

    @pytest.mark.asyncio
    async def test_health_check_degraded_loading_model(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value={"status": "loading model"})
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.DEGRADED
        assert status.reason == "loading model"
        assert status.healthy is True  # degraded counts as healthy via descriptor

    @pytest.mark.asyncio
    async def test_health_check_unavailable_on_error(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value={"status": "error"})
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
        # Second call must not raise and must leave state cleared.
        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_generate_batch(self, adapter):
        adapter._initialized = True
        adapter._cfg = LlamaCppConfig.from_dict({})
        adapter._client = MagicMock()
        # generate_batch uses asyncio.to_thread(client.chat_completions, ...)
        # so the mock must be sync and return an object with a .body attribute.
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
        assert results[1].text == "A"
        assert results[0].tokens_generated == 1
        assert adapter._requests_served == before + 2

    @pytest.mark.asyncio
    async def test_tokenize(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.tokenize = AsyncMock(return_value=[1, 2, 3, 4])
        ids = await adapter.tokenize("hi there")
        assert ids == [1, 2, 3, 4]
        assert len(ids) == 4
        assert all(isinstance(t, int) for t in ids)
