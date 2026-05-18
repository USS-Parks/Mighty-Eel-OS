"""Unit tests for MAI TensorRT-LLM adapter."""
from __future__ import annotations

from unittest.mock import AsyncMock

import pytest

from adapters.base import GenerationParams, UnsupportedOperationError
from adapters.tensorrt.adapter import TensorRtAdapter
from adapters.tensorrt.config import TensorRtConfig


@pytest.fixture
def config():
    return {"host": "127.0.0.1", "port": 8000, "engine_dir": "/models/llama-trt"}


@pytest.fixture
def adapter(config):
    return TensorRtAdapter(config)


class TestTensorRtConfig:
    def test_defaults(self):
        cfg = TensorRtConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8001
        assert cfg.grpc_port == 8002
        assert cfg.enable_inflight_batching is True

    def test_custom(self):
        cfg = TensorRtConfig.from_dict({"precision": "int8", "max_batch_size": 64})
        assert cfg.precision == "int8"
        assert cfg.max_batch_size == 64


class TestTensorRtAdapter:
    @pytest.mark.asyncio
    async def test_initialize(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.model_ready = AsyncMock(return_value=True)
        adapter._client.server_metadata = AsyncMock(return_value={
            "name": "triton", "version": "2.40.0"
        })
        adapter._cfg = TensorRtConfig.from_dict({"engine_dir": "/models/test"})
        await adapter.initialize()
        assert adapter._initialized is True

    @pytest.mark.asyncio
    async def test_generate(self, adapter):
        adapter._initialized = True
        adapter._cfg = TensorRtConfig.from_dict({})
        adapter._client = AsyncMock()
        adapter._model_id = "tensorrt_llm"
        adapter._engine_ready = True
        adapter._client.generate = AsyncMock(return_value={
            "text_output": "Generated text",
            "output_tokens": 10,
        })
        result = await adapter.generate("Hi", GenerationParams())
        assert "Generated" in result.text

    @pytest.mark.asyncio
    async def test_embed_raises(self, adapter):
        adapter._initialized = True
        with pytest.raises(UnsupportedOperationError):
            await adapter.embed(["hello"])

    def test_capabilities(self, adapter):
        adapter._cfg = TensorRtConfig.from_dict({})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_embeddings is False
        assert caps.extra["inflight_batching"] is True

    @pytest.mark.asyncio
    async def test_health_degraded_no_engine(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._engine_ready = False
        status = await adapter.health_check()
        assert status.healthy is True
        assert "degraded" in status.message.lower() or "engine" in status.message.lower()
