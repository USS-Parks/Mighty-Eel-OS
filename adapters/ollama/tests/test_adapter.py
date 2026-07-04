"""Unit tests for Ollama adapter with mocked HTTP.

Tests cover: initialization, generate (streaming), generate_batch,
embed, health_check, capabilities, error mapping, model switching.

"""

from __future__ import annotations

from typing import Any
from unittest.mock import patch

import pytest

from adapters.base import (
    BackendUnavailableError,
    GenerationParams,
    HealthStatusKind,
    ModelNotFoundError,
    ValidationError,
)
from adapters.ollama.adapter import OllamaAdapter
from adapters.ollama.client import OllamaClient, OllamaResponse, OllamaStreamChunk
from adapters.ollama.config import OllamaConfig

# ─── Fixtures ────────────────────────────────────────────────────────────────


@pytest.fixture
def config() -> OllamaConfig:
    return OllamaConfig(
        host="127.0.0.1",
        port=11434,
        default_model="llama3.1:8b-instruct-q4_K_M",
        embedding_model="nomic-embed-text",
    )


@pytest.fixture
def adapter() -> OllamaAdapter:
    return OllamaAdapter()


async def _consume_stream(stream: Any) -> None:
    async for _tok in stream:
        pass


# ─── Config Tests ────────────────────────────────────────────────────────────


class TestOllamaConfig:
    def test_defaults(self) -> None:
        config = OllamaConfig()
        assert config.host == "127.0.0.1"
        assert config.port == 11434
        assert config.base_url == "http://127.0.0.1:11434"
        assert config.allow_pull is False  # Air-gapped by default

    def test_from_dict(self) -> None:
        data = {"host": "localhost", "port": 9999, "custom_key": "value"}
        config = OllamaConfig.from_dict(data)
        assert config.host == "localhost"
        assert config.port == 9999
        assert config.extra_options == {"custom_key": "value"}

    def test_base_url(self) -> None:
        config = OllamaConfig(host="10.0.0.1", port=8080)
        assert config.base_url == "http://10.0.0.1:8080"


# ─── Client Tests ────────────────────────────────────────────────────────────


class TestOllamaClient:
    def test_health_success(self, config: OllamaConfig) -> None:
        client = OllamaClient(config)
        with patch.object(client, "_request") as mock_req:
            mock_req.return_value = OllamaResponse(
                status_code=200, body={}, elapsed_ms=5.0,
            )
            assert client.health() is True

    def test_health_failure(self, config: OllamaConfig) -> None:
        client = OllamaClient(config)
        with patch.object(client, "_request") as mock_req:
            mock_req.side_effect = BackendUnavailableError()
            assert client.health() is False

    def test_list_models(self, config: OllamaConfig) -> None:
        client = OllamaClient(config)
        with patch.object(client, "_request") as mock_req:
            mock_req.return_value = OllamaResponse(
                status_code=200,
                body={"models": [{"name": "llama3.1:8b"}, {"name": "mistral:7b"}]},
                elapsed_ms=10.0,
            )
            models = client.list_models()
            assert len(models) == 2
            assert models[0]["name"] == "llama3.1:8b"

    def test_embed(self, config: OllamaConfig) -> None:
        client = OllamaClient(config)
        with patch.object(client, "_request") as mock_req:
            mock_req.return_value = OllamaResponse(
                status_code=200,
                body={"embeddings": [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]]},
                elapsed_ms=15.0,
            )
            vectors = client.embed("nomic-embed-text", ["hello", "world"])
            assert len(vectors) == 2
            assert vectors[0] == [0.1, 0.2, 0.3]

    def test_pull_disabled_air_gap(self, config: OllamaConfig) -> None:
        client = OllamaClient(config)
        # allow_pull defaults to False (air-gapped)
        assert client.pull_model("some-model") is False


# ─── Adapter Tests ───────────────────────────────────────────────────────────


class TestOllamaAdapter:
    @pytest.mark.asyncio
    async def test_initialize_success(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            handle = await adapter.initialize({}, hil_handle=None)
            assert handle == "ollama:llama3.1:8b-instruct-q4_K_M"

    @pytest.mark.asyncio
    async def test_initialize_server_unavailable(
        self, adapter: OllamaAdapter,
    ) -> None:
        with patch.object(OllamaClient, "health", return_value=False):
            with pytest.raises(BackendUnavailableError):
                await adapter.initialize({}, hil_handle=None)

    @pytest.mark.asyncio
    async def test_generate_rejects_empty_prompt(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        with pytest.raises(ValidationError):
            await _consume_stream(adapter.generate("   ", GenerationParams()))

    @pytest.mark.asyncio
    async def test_generate_streaming(self, adapter: OllamaAdapter) -> None:
        # Initialize first
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        # Mock streaming
        chunks = [
            OllamaStreamChunk(content="Hello", done=False),
            OllamaStreamChunk(content=" world", done=True),
        ]

        with patch.object(adapter, "_collect_stream", return_value=chunks):
            tokens: list[Any] = []
            async for token in adapter.generate("Hi", GenerationParams()):
                tokens.append(token)

            assert len(tokens) >= 2
            assert tokens[0].text == "Hello"
            assert tokens[1].text == " world"

    @pytest.mark.asyncio
    async def test_generate_batch(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        mock_resp = OllamaResponse(
            status_code=200,
            body={"response": "Answer", "eval_count": 5, "done_reason": "stop"},
            elapsed_ms=100.0,
        )
        with patch.object(OllamaClient, "generate_completion", return_value=mock_resp):
            results = await adapter.generate_batch(
                ["Q1", "Q2"], GenerationParams(),
            )
            assert len(results) == 2
            assert results[0].text == "Answer"
            assert results[0].tokens_generated == 5

    @pytest.mark.asyncio
    async def test_embed(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        with patch.object(OllamaClient, "embed", return_value=[[0.1, 0.2], [0.3, 0.4]]):
            embeddings = await adapter.embed(["hello", "world"])
            assert len(embeddings) == 2
            assert embeddings[0].vector == [0.1, 0.2]

    @pytest.mark.asyncio
    async def test_health_check_healthy(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[]):
            await adapter.initialize({}, hil_handle=None)

        with patch.object(OllamaClient, "health", return_value=True):
            status = await adapter.health_check()
            assert status.kind == HealthStatusKind.HEALTHY

    @pytest.mark.asyncio
    async def test_health_check_unavailable(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[]):
            await adapter.initialize({}, hil_handle=None)

        with patch.object(OllamaClient, "health", return_value=False):
            status = await adapter.health_check()
            assert status.kind == HealthStatusKind.UNAVAILABLE

    def test_capabilities(self, adapter: OllamaAdapter) -> None:
        caps = adapter.capabilities()
        assert caps.max_context_window == 131072
        assert caps.supports_streaming is True
        assert caps.supports_batching is False
        assert caps.supports_embedding is True
        assert caps.supports_vision is False

    @pytest.mark.asyncio
    async def test_switch_model(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
                 {"name": "mistral:7b"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        with patch.object(OllamaClient, "list_models", return_value=[
            {"name": "llama3.1:8b-instruct-q4_K_M"},
            {"name": "mistral:7b"},
        ]):
            await adapter.switch_model("mistral:7b")
            assert adapter._model == "mistral:7b"

    @pytest.mark.asyncio
    async def test_switch_model_not_found(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[
                 {"name": "llama3.1:8b-instruct-q4_K_M"},
             ]):
            await adapter.initialize({}, hil_handle=None)

        with patch.object(OllamaClient, "list_models", return_value=[
            {"name": "llama3.1:8b-instruct-q4_K_M"},
        ]):
            with pytest.raises(ModelNotFoundError):
                await adapter.switch_model("nonexistent:latest")

    @pytest.mark.asyncio
    async def test_shutdown(self, adapter: OllamaAdapter) -> None:
        with patch.object(OllamaClient, "health", return_value=True), \
             patch.object(OllamaClient, "list_models", return_value=[]):
            await adapter.initialize({}, hil_handle=None)
            assert adapter._initialized is True

        await adapter.shutdown()
        assert adapter._initialized is False
        assert adapter._client is None


# ─── Runner Tests ────────────────────────────────────────────────────────────


class TestRunner:
    """Basic tests for the runner protocol handler."""

    @pytest.mark.asyncio
    async def test_load_adapter(self) -> None:
        from adapters.runner import load_adapter
        adapter = load_adapter("adapters.ollama.adapter", "OllamaAdapter")
        assert isinstance(adapter, OllamaAdapter)


# ─── J-12: async context manager smoke ───────────────────────────────────────


@pytest.mark.asyncio
async def test_async_context_manager_lifecycle_j12() -> None:
    """J-12: ``async with`` calls initialize on enter, shutdown on exit."""
    from unittest.mock import AsyncMock

    from adapters.base import ValidationError

    adapter = OllamaAdapter()
    adapter.initialize = AsyncMock(return_value=None)
    adapter.shutdown = AsyncMock(return_value=None)
    adapter.set_config({"host": "127.0.0.1"}, hil_handle=None)
    async with adapter as bound:
        assert bound is adapter
    adapter.initialize.assert_awaited_once_with(
        {"host": "127.0.0.1"}, hil_handle=None,
    )
    adapter.shutdown.assert_awaited_once()

    fresh = OllamaAdapter()
    with pytest.raises(ValidationError, match="config not set"):
        async with fresh:
            pass
