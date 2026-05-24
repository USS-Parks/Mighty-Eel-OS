"""Unit tests for the MAI MLX adapter (J-25).

These tests must pass on non-Apple CI. The strategy:

  1. We never import the real `mlx_lm` package — instead, every test
     constructs an `MLXClient` with `mlx_module=<fake_module>`, which
     bypasses the platform check and the import path.
  2. Live-backend coverage lives in `test_integration_live.py`, gated
     by `MLX_MODEL_PATH` plus a runtime Apple Silicon check.

The assertion floor is set per ADAPTER-TEST-HARNESS-LOCK.md §Unit Test
Minimums; every required behavior has at least one meaningful assert.
"""

from __future__ import annotations

import asyncio
from collections.abc import Iterator
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    FinishReason,
    GenerationParams,
    HealthStatusKind,
    ModelNotFoundError,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.mlx.adapter import MLXAdapter
from adapters.mlx.client import MLXClient, MLXLoadError
from adapters.mlx.config import MLXConfig

# ─── Fake mlx_lm module factory ─────────────────────────────────────────────


def _fake_mlx_module(
    *,
    generate_text: str = "Hello world",
    stream_chunks: list[str] | None = None,
    load_raises: BaseException | None = None,
    version: str = "0.20.fake",
) -> SimpleNamespace:
    """Build a duck-typed stand-in for the real `mlx_lm` package.

    The real package exposes `load`, `generate`, `stream_generate`, and a
    `__version__` attribute. We mimic the minimum surface used by the
    adapter; nothing else is touched.
    """
    chunks = stream_chunks if stream_chunks is not None else ["Hello", " ", "world"]

    fake_tokenizer = MagicMock()
    fake_tokenizer.encode = MagicMock(return_value=[1, 2, 3])
    fake_model = MagicMock(name="FakeMLXModel")

    def fake_load(path: str, tokenizer_config: dict[str, Any] | None = None):
        if load_raises is not None:
            raise load_raises
        return fake_model, fake_tokenizer

    def fake_generate(_m, _t, *, prompt, max_tokens, temp, top_p):
        return generate_text

    def fake_stream_generate(_m, _t, *, prompt, max_tokens, temp, top_p) -> Iterator[str]:
        yield from chunks

    return SimpleNamespace(
        load=fake_load,
        generate=fake_generate,
        stream_generate=fake_stream_generate,
        __version__=version,
    )


@pytest.fixture
def fake_mlx():
    return _fake_mlx_module()


@pytest.fixture
def loaded_adapter(fake_mlx):
    """An MLXAdapter that has been initialized against a fake mlx-lm."""
    adapter = MLXAdapter({"model_path": "/fake/model", "timeout_ms": 5000})
    adapter._client = MLXClient(model_path="/fake/model", mlx_module=fake_mlx)
    adapter._config = MLXConfig.from_dict({"model_path": "/fake/model"})
    adapter._client.load()
    adapter._initialized = True
    adapter._start_time_ms = 1
    return adapter


# ─── Config tests ───────────────────────────────────────────────────────────


class TestMLXConfig:
    def test_defaults(self):
        cfg = MLXConfig.from_dict({})
        assert cfg.model_path == ""
        assert cfg.max_tokens == 512
        assert cfg.temperature == 0.7
        assert cfg.top_p == 0.9
        assert cfg.timeout_ms == 60_000
        assert cfg.stream_timeout_ms == 300_000
        assert cfg.max_batch_size == 4
        assert cfg.max_context_window == 8192

    def test_overrides(self):
        cfg = MLXConfig.from_dict(
            {"model_path": "/m", "max_tokens": 64, "temperature": 0.1},
        )
        assert cfg.model_path == "/m"
        assert cfg.max_tokens == 64
        assert cfg.temperature == 0.1

    def test_extra_options_collected(self):
        cfg = MLXConfig.from_dict({"unknown_flag": 7, "model_path": "/m"})
        assert cfg.extra_options == {"unknown_flag": 7}
        assert cfg.model_path == "/m"


# ─── Client tests ───────────────────────────────────────────────────────────


class TestMLXClient:
    def test_construction_does_not_call_network(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        assert client.loaded is False
        assert client.backend_version == "unknown"

    def test_load_happy_path(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        client.load()
        assert client.loaded is True
        assert client.backend_version == "0.20.fake"

    def test_load_is_idempotent(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        client.load()
        client.load()  # must not raise
        assert client.loaded is True

    def test_load_empty_path_raises(self, fake_mlx):
        client = MLXClient(model_path="", mlx_module=fake_mlx)
        with pytest.raises(MLXLoadError):
            client.load()

    def test_load_missing_path_raises(self):
        fake = _fake_mlx_module(load_raises=FileNotFoundError("no such file"))
        client = MLXClient(model_path="/missing", mlx_module=fake)
        with pytest.raises(MLXLoadError) as exc_info:
            client.load()
        assert "not found" in str(exc_info.value).lower()

    def test_generate_returns_text_tokens_hitmax(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        client.load()
        text, tokens, hit_max = client.generate(
            "Q", max_tokens=10, temperature=0.5, top_p=0.9,
        )
        assert text == "Hello world"
        assert tokens == 3  # fake tokenizer encode -> [1,2,3]
        assert hit_max is False

    def test_generate_hits_max(self):
        fake = _fake_mlx_module(generate_text="x" * 200)
        # Tokenizer encode returns [1,2,3] (len 3), so hit_max only when
        # max_tokens <= 3 — drive that explicitly.
        client = MLXClient(model_path="/m", mlx_module=fake)
        client.load()
        _, tokens, hit_max = client.generate(
            "Q", max_tokens=3, temperature=0.0, top_p=1.0,
        )
        assert hit_max is True
        assert tokens >= 3

    def test_generate_before_load_raises(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        with pytest.raises(MLXLoadError):
            client.generate("Q", max_tokens=10, temperature=0.5, top_p=1.0)

    def test_stream_generate_order_preserved(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        client.load()
        out = list(client.stream_generate(
            "Q", max_tokens=10, temperature=0.7, top_p=0.9,
        ))
        assert out == ["Hello", " ", "world"]

    def test_stream_generate_handles_token_objects(self):
        chunks = [SimpleNamespace(text="a"), SimpleNamespace(text="b")]
        fake = _fake_mlx_module(stream_chunks=chunks)
        client = MLXClient(model_path="/m", mlx_module=fake)
        client.load()
        out = list(client.stream_generate(
            "Q", max_tokens=10, temperature=0.7, top_p=0.9,
        ))
        assert out == ["a", "b"]

    def test_close_is_idempotent(self, fake_mlx):
        client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        client.load()
        client.close()
        client.close()  # must not raise
        assert client.loaded is False


# ─── Adapter tests ──────────────────────────────────────────────────────────


class TestMLXAdapterLifecycle:
    @pytest.mark.asyncio
    async def test_initialize_validates_model_path(self):
        adapter = MLXAdapter({})
        with pytest.raises(ValidationError):
            await adapter.initialize()
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_initialize_happy_path(self, fake_mlx):
        adapter = MLXAdapter({"model_path": "/m"})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        handle = await adapter.initialize()
        assert isinstance(handle, str)
        assert handle.startswith("mlx-")
        assert adapter._initialized is True

    @pytest.mark.asyncio
    async def test_initialize_backend_unavailable(self):
        fake = _fake_mlx_module(load_raises=MLXLoadError("mlx-lm not installed"))
        adapter = MLXAdapter({"model_path": "/m"})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake)
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_initialize_model_not_found(self):
        fake = _fake_mlx_module(load_raises=FileNotFoundError("no such file"))
        adapter = MLXAdapter({"model_path": "/missing"})
        adapter._client = MLXClient(model_path="/missing", mlx_module=fake)
        with pytest.raises(ModelNotFoundError):
            await adapter.initialize()

    @pytest.mark.asyncio
    async def test_shutdown_is_idempotent(self, loaded_adapter):
        await loaded_adapter.shutdown()
        await loaded_adapter.shutdown()  # must not raise
        assert loaded_adapter._initialized is False
        assert loaded_adapter._client is None

    @pytest.mark.asyncio
    async def test_call_before_init_raises(self):
        adapter = MLXAdapter({"model_path": "/m"})
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams())
        with pytest.raises(BackendUnavailableError):
            await adapter.generate_batch(["a"], GenerationParams())


class TestMLXAdapterGeneration:
    @pytest.mark.asyncio
    async def test_generate_non_streaming(self, loaded_adapter):
        before = loaded_adapter._requests_served
        result = await loaded_adapter.generate("hi", GenerationParams(max_tokens=10))
        assert result.text == "Hello world"
        assert result.tokens_generated == 3
        assert result.finish_reason == FinishReason.STOP
        assert loaded_adapter._requests_served == before + 1

    @pytest.mark.asyncio
    async def test_generate_max_tokens_finish(self):
        fake = _fake_mlx_module(generate_text="cut")
        adapter = MLXAdapter({"model_path": "/m", "timeout_ms": 5000})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake)
        adapter._config = MLXConfig.from_dict({"model_path": "/m"})
        adapter._client.load()
        adapter._initialized = True
        adapter._start_time_ms = 1

        result = await adapter.generate("hi", GenerationParams(max_tokens=2))
        assert result.finish_reason == FinishReason.MAX_TOKENS

    @pytest.mark.asyncio
    async def test_generate_streaming_order_and_terminal(self, loaded_adapter):
        agen = await loaded_adapter.generate(
            "hi", GenerationParams(max_tokens=10), stream=True,
        )
        tokens = []
        async for t in agen:
            tokens.append(t)
        # 3 content + 1 terminal sentinel
        assert [t.text for t in tokens] == ["Hello", " ", "world", ""]
        assert tokens[-1].is_end_of_text is True
        assert all(t.index == i for i, t in enumerate(tokens))

    @pytest.mark.asyncio
    async def test_generate_streaming_drops_empty_chunks(self):
        fake = _fake_mlx_module(stream_chunks=["a", "", "b", ""])
        adapter = MLXAdapter({"model_path": "/m"})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake)
        adapter._config = MLXConfig.from_dict({"model_path": "/m"})
        adapter._client.load()
        adapter._initialized = True
        adapter._start_time_ms = 1

        agen = await adapter.generate("hi", GenerationParams(), stream=True)
        out = [t.text async for t in agen]
        # Empty chunks dropped; one terminal sentinel emitted.
        assert out == ["a", "b", ""]

    @pytest.mark.asyncio
    async def test_generate_timeout_maps_to_typed_error(self, monkeypatch):
        fake = _fake_mlx_module()
        adapter = MLXAdapter({"model_path": "/m", "timeout_ms": 50})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake)
        adapter._config = MLXConfig.from_dict({"model_path": "/m", "timeout_ms": 50})
        adapter._client.load()
        adapter._initialized = True
        adapter._start_time_ms = 1

        async def slow_wait(coro: Any, *_a: Any, **_k: Any) -> None:
            # Close the wrapped coroutine so it does not leak un-awaited.
            coro.close()
            raise TimeoutError

        monkeypatch.setattr(asyncio, "wait_for", slow_wait)
        with pytest.raises(AdapterTimeoutError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_backend_crash_during_call(self):
        def boom(*_a: Any, **_k: Any) -> None:
            raise MLXLoadError("client not loaded")

        fake = _fake_mlx_module()
        fake.generate = boom
        adapter = MLXAdapter({"model_path": "/m"})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake)
        adapter._config = MLXConfig.from_dict({"model_path": "/m"})
        adapter._client.load()
        adapter._initialized = True
        adapter._start_time_ms = 1

        with pytest.raises(BackendUnavailableError):
            await adapter.generate("hi", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_batch_preserves_order(self, loaded_adapter):
        results = await loaded_adapter.generate_batch(
            ["one", "two", "three"], GenerationParams(),
        )
        assert len(results) == 3
        for r in results:
            assert r.text == "Hello world"

    @pytest.mark.asyncio
    async def test_generate_batch_empty(self, loaded_adapter):
        results = await loaded_adapter.generate_batch([], GenerationParams())
        assert results == []


class TestMLXAdapterUnsupported:
    @pytest.mark.asyncio
    async def test_embed_raises_unsupported(self, loaded_adapter):
        with pytest.raises(UnsupportedOperationError) as exc_info:
            await loaded_adapter.embed(["hi"])
        assert "embed" in str(exc_info.value).lower()


class TestMLXAdapterHealth:
    @pytest.mark.asyncio
    async def test_health_before_init(self):
        adapter = MLXAdapter({"model_path": "/m"})
        h = await adapter.health_check()
        assert h.kind == HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_health_healthy(self, loaded_adapter):
        h = await loaded_adapter.health_check()
        assert h.kind == HealthStatusKind.HEALTHY
        assert h.healthy is True

    @pytest.mark.asyncio
    async def test_health_degraded_when_model_lost(self, loaded_adapter):
        loaded_adapter._client.close()
        h = await loaded_adapter.health_check()
        assert h.kind == HealthStatusKind.DEGRADED
        assert h.reason is not None


class TestMLXAdapterCapabilities:
    def test_capabilities_truthful(self):
        adapter = MLXAdapter({"model_path": "/m"})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_embedding is False
        assert caps.supports_tool_calling is False
        assert caps.supports_vision is False
        assert caps.supports_structured_output is False
        assert caps.supports_continuous_batching is False
        assert caps.supports_hot_swap is False
        assert "apple_silicon_only" in caps.extra
        assert caps.extra["apple_silicon_only"] is True

    def test_capabilities_default_context_window(self):
        adapter = MLXAdapter({"model_path": "/m"})
        caps = adapter.capabilities()
        assert caps.max_context_window == 8192

    def test_capabilities_backend_version_unknown_pre_init(self):
        adapter = MLXAdapter({"model_path": "/m"})
        caps = adapter.capabilities()
        assert caps.backend_version == "unknown"

    def test_capabilities_backend_version_after_load(self, fake_mlx):
        adapter = MLXAdapter({"model_path": "/m"})
        adapter._client = MLXClient(model_path="/m", mlx_module=fake_mlx)
        adapter._client.load()
        caps = adapter.capabilities()
        assert caps.backend_version == "0.20.fake"
