"""Unit tests for MAI llama.cpp adapter."""
from __future__ import annotations

from unittest.mock import AsyncMock

import pytest

from adapters.base import GenerationParams, UnsupportedOperationError
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
        await adapter.initialize()
        assert adapter._initialized is True

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
        result = await adapter.generate("Hi", GenerationParams())
        assert result.text == "Response"
        assert result.tokens_generated == 5

    @pytest.mark.asyncio
    async def test_embed_raises(self, adapter):
        adapter._initialized = True
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])

    def test_capabilities(self, adapter):
        adapter._cfg = LlamaCppConfig.from_dict({})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_embeddings is False
        assert caps.supports_structured_output is True

    @pytest.mark.asyncio
    async def test_health_check_healthy(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model_id = "test.gguf"
        adapter._client.health = AsyncMock(return_value={"status": "ok"})
        status = await adapter.health_check()
        assert status.healthy is True
