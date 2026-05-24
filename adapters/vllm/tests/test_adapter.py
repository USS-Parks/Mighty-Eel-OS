"""Unit tests for MAI vLLM adapter.

J-18 (DOUGHERTY lane) grew the assertion floor from 16 to 60+ and closed
the embed return-type bug surfaced in
`docs/ADAPTER-COMPLETION-MATRIX.md` §3 #1 (the original
`result[0] == [0.1, 0.2, 0.3]` assertion only passed because
`Embedding.__eq__` is permissive against lists; the canonical assertion
is now on `.vector` / `.input_tokens`). Streaming is exercised
end-to-end against a real localhost HTTP/SSE server
(`adapters/tests/_streaming_server.py`) so the client's `urllib` call,
the `for line in resp:` SSE parser, and the adapter's
`_generate_stream` loop are all driven by real bytes — not by
`AsyncMock`. Live-backend coverage against a real vLLM server still
lives in `test_integration_live.py`; that is a different, opt-in
concern gated by `VLLM_HOST`.
"""
from __future__ import annotations

import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    ContextExceededError,
    Embedding,
    FinishReason,
    GenerationParams,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
)
from adapters.tests._streaming_server import StreamRecipe, streaming_server
from adapters.vllm.adapter import VllmAdapter
from adapters.vllm.client import VllmClient
from adapters.vllm.config import VllmConfig


@pytest.fixture
def config():
    return {
        "host": "127.0.0.1",
        "port": 8000,
        "tensor_parallel_size": 2,
        "enable_lora": True,
    }


@pytest.fixture
def adapter(config):
    return VllmAdapter(config)


class TestVllmConfig:
    def test_defaults(self):
        cfg = VllmConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8000
        assert cfg.tensor_parallel_size == 1
        assert cfg.enable_lora is False

    def test_custom(self):
        cfg = VllmConfig.from_dict({"port": 9000, "quantization": "awq"})
        assert cfg.port == 9000
        assert cfg.quantization == "awq"


class TestVllmAdapter:
    @pytest.mark.asyncio
    async def test_initialize(self, adapter):
        mock_resp = {"data": [{"id": "meta-llama/Llama-3-70B"}]}
        with patch.object(adapter, "_client") as _:
            adapter._client = AsyncMock()
            adapter._client.health = AsyncMock(return_value=True)
            adapter._client.models = AsyncMock(return_value=mock_resp)
            adapter._cfg = VllmConfig.from_dict({})
            await adapter.initialize()
            assert adapter._initialized is True

    @pytest.mark.asyncio
    async def test_initialize_backend_unavailable(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=False)
        adapter._cfg = VllmConfig.from_dict({})
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize()
        assert adapter._initialized is False
        assert adapter._requests_served == 0

    @pytest.mark.asyncio
    async def test_initialize_picks_partial_match_model(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.models = AsyncMock(return_value={"data": [
            {"id": "meta-llama/Llama-3-70B-Instruct"},
        ]})
        adapter._cfg = VllmConfig.from_dict({"default_model": "Llama-3-70B"})
        await adapter.initialize()
        # Partial-match logic resolves the configured stem to the actual model id.
        assert adapter._model == "meta-llama/Llama-3-70B-Instruct"
        assert adapter._available_models == ["meta-llama/Llama-3-70B-Instruct"]

    @pytest.mark.asyncio
    async def test_initialize_falls_back_to_first_model(self, adapter):
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.models = AsyncMock(return_value={"data": [
            {"id": "mistralai/Mixtral-8x7B-Instruct"},
            {"id": "meta-llama/Llama-3-70B-Instruct"},
        ]})
        # Configured default does not appear and has no partial overlap.
        adapter._cfg = VllmConfig.from_dict({"default_model": "completely-other-model"})
        await adapter.initialize()
        assert adapter._model == "mistralai/Mixtral-8x7B-Instruct"

    @pytest.mark.asyncio
    async def test_generate(self, adapter):
        adapter._initialized = True
        adapter._cfg = VllmConfig.from_dict({})
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "Hello!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2},
        })
        before = adapter._requests_served
        result = await adapter.generate("Hi", GenerationParams())
        assert result.text == "Hello!"
        assert result.tokens_generated == 2
        assert result.finish_reason == FinishReason.STOP
        assert adapter._requests_served == before + 1

    @pytest.mark.asyncio
    async def test_generate_max_tokens_finish(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
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
        adapter._model = "test-model"
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
    async def test_generate_propagates_client_timeout(self, adapter):
        # ADAPTER-SHARED-CONTRACT §"Error Mapping": read timeout from
        # the HTTP layer must surface as AdapterTimeoutError, unchanged,
        # across the adapter boundary.
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.chat_completions = AsyncMock(
            side_effect=AdapterTimeoutError(timeout_ms=1000),
        )
        with pytest.raises(AdapterTimeoutError):
            await adapter.generate("Q", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_propagates_model_not_found(self, adapter):
        # ADAPTER-SHARED-CONTRACT §"Error Mapping": 404 from backend
        # must surface as ModelNotFoundError, not a raw backend
        # exception or a fake success.
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.chat_completions = AsyncMock(
            side_effect=ModelNotFoundError(model="missing"),
        )
        with pytest.raises(ModelNotFoundError):
            await adapter.generate("Q", GenerationParams())

    @pytest.mark.asyncio
    async def test_generate_malformed_nonstream_response_degrades(self, adapter):
        # ADAPTER-SHARED-CONTRACT §"Generation": pins the current
        # nonstream malformed-response behavior — the adapter does not
        # raise on a body missing "choices"; it returns an empty
        # GenerationResult. The harness lock requires this case have a
        # test; the Known Limitations section of the evidence note
        # flags this as the documented degradation (vs typed-error
        # raise) so a future convergence pass can decide whether to
        # tighten the contract.
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.chat_completions = AsyncMock(return_value={
            # No "choices" key, no "usage" key — completely malformed.
            "unrelated": "garbage",
        })
        result = await adapter.generate("Q", GenerationParams())
        assert result.text == ""
        assert result.tokens_generated == 0
        assert result.finish_reason == FinishReason.STOP

    @pytest.mark.asyncio
    async def test_initialize_reuses_client_across_calls(self, adapter):
        # ADAPTER-SHARED-CONTRACT §"HTTP And Session Pooling": prove
        # at least two calls reuse the same client object — no
        # per-request client construction.
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=True)
        adapter._client.models = AsyncMock(return_value={"data": [
            {"id": "model-a"},
        ]})
        adapter._cfg = VllmConfig.from_dict({})
        await adapter.initialize()
        first_client_id = id(adapter._client)

        # A second initialize call (idempotent re-init) must not
        # replace the client. The adapter's guard is
        # `if self._client is None: self._client = VllmClient(...)`.
        await adapter.initialize()
        assert id(adapter._client) == first_client_id

        # Two generate calls reuse the same client instance.
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": "x"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens": 1},
        })
        await adapter.generate("a", GenerationParams())
        await adapter.generate("b", GenerationParams())
        assert id(adapter._client) == first_client_id
        # And both calls landed on the SAME mock — proves reuse.
        assert adapter._client.chat_completions.await_count == 2

    @pytest.mark.asyncio
    async def test_generate_passes_guided_json_for_structured_output(self, adapter):
        # vLLM advertises supports_structured_output=True and the adapter
        # routes structured_schema -> guided_json. Verify the kwarg is
        # forwarded into the chat_completions call.
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.chat_completions = AsyncMock(return_value={
            "choices": [{"message": {"content": '{"ok": true}'}, "finish_reason": "stop"}],
            "usage": {"completion_tokens": 4},
        })
        schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}}
        params = GenerationParams(structured_schema=schema)
        result = await adapter.generate("describe ok", params)
        assert result.text == '{"ok": true}'
        # AsyncMock records call kwargs — guided_json must be present and
        # equal to the schema we passed in.
        call_kwargs = adapter._client.chat_completions.call_args.kwargs
        assert call_kwargs.get("guided_json") == schema
        assert call_kwargs.get("stream") is False

    @pytest.mark.asyncio
    async def test_generate_batch(self, adapter):
        adapter._initialized = True
        adapter._cfg = VllmConfig.from_dict({})
        adapter._client = MagicMock()
        adapter._model = "test-model"
        # generate_batch routes through asyncio.to_thread(client.chat_completions, ...)
        # so the mock must be sync and return an object with a .body attribute.
        resp = MagicMock()
        resp.body = {
            "choices": [{"message": {"content": "A"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens": 1},
        }
        adapter._client.chat_completions = MagicMock(return_value=resp)
        before = adapter._requests_served
        results = await adapter.generate_batch(["Q1", "Q2", "Q3"], GenerationParams())
        assert len(results) == 3
        assert results[0].text == "A"
        assert results[1].text == "A"
        assert results[2].tokens_generated == 1
        assert adapter._requests_served == before + 3

    @pytest.mark.asyncio
    async def test_generate_batch_handles_empty_choices_per_item(self, adapter):
        adapter._initialized = True
        adapter._client = MagicMock()
        adapter._model = "test-model"
        resp = MagicMock()
        resp.body = {"choices": []}
        adapter._client.chat_completions = MagicMock(return_value=resp)
        results = await adapter.generate_batch(["only-one"], GenerationParams())
        assert len(results) == 1
        assert results[0].text == ""
        assert results[0].tokens_generated == 0

    @pytest.mark.asyncio
    async def test_health_check_healthy(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._model = "test-model"
        adapter._client.health = AsyncMock(return_value=True)
        status = await adapter.health_check()
        assert status.healthy is True
        assert status.kind == HealthStatusKind.HEALTHY
        assert status.uptime_ms >= 0
        assert status.requests_served >= 0

    @pytest.mark.asyncio
    async def test_health_check_unavailable_when_uninitialized(self, adapter):
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert bool(status.healthy) is False

    @pytest.mark.asyncio
    async def test_health_check_unavailable_when_probe_fails(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.health = AsyncMock(return_value=False)
        status = await adapter.health_check()
        assert status.kind == HealthStatusKind.UNAVAILABLE
        assert bool(status.healthy) is False

    def test_capabilities(self, adapter):
        adapter._cfg = VllmConfig.from_dict({"enable_lora": True})
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_batching is True
        assert caps.supports_continuous_batching is True
        assert caps.supports_structured_output is True
        assert caps.supports_tool_calling is True
        assert caps.supports_embedding is True
        # Property alias must agree with the canonical singular field.
        assert caps.supports_embeddings == caps.supports_embedding
        assert caps.supports_hot_swap is True
        assert caps.supports_vision is False
        assert caps.max_context_window == 32768
        assert "awq" in caps.supported_quantizations
        assert "fp8" in caps.supported_quantizations
        assert caps.extra["lora"] is True

    def test_capabilities_reflect_disabled_lora(self, adapter):
        adapter._cfg = VllmConfig.from_dict({"enable_lora": False})
        caps = adapter.capabilities()
        assert caps.extra["lora"] is False

    @pytest.mark.asyncio
    async def test_embed_returns_embedding_dataclass(self, adapter):
        # J-18: was previously asserting against the literal list, which
        # only passed because Embedding.__eq__ is permissive against
        # lists. The canonical assertion is on .vector / .input_tokens.
        adapter._initialized = True
        adapter._cfg = VllmConfig.from_dict({})
        adapter._client = AsyncMock()
        adapter._client.embeddings = AsyncMock(return_value={
            "data": [{"embedding": [0.1, 0.2, 0.3]}],
            "usage": {"total_tokens": 8},
        })
        result = await adapter.embed(["hello"])
        assert len(result) == 1
        assert isinstance(result[0], Embedding)
        assert result[0].vector == [0.1, 0.2, 0.3]
        assert result[0].input_tokens == 8  # total_tokens / len(texts)

    @pytest.mark.asyncio
    async def test_embed_distributes_tokens_across_inputs(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.embeddings = AsyncMock(return_value={
            "data": [
                {"embedding": [0.1, 0.2]},
                {"embedding": [0.3, 0.4]},
                {"embedding": [0.5, 0.6]},
            ],
            "usage": {"total_tokens": 9},
        })
        result = await adapter.embed(["a", "b", "c"])
        assert len(result) == 3
        # 9 / 3 = 3 per text under the adapter's even-split heuristic.
        for emb in result:
            assert isinstance(emb, Embedding)
            assert emb.input_tokens == 3
        assert result[0].vector == [0.1, 0.2]
        assert result[2].vector == [0.5, 0.6]

    @pytest.mark.asyncio
    async def test_embed_falls_back_to_char_estimate_when_usage_absent(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.embeddings = AsyncMock(return_value={
            "data": [{"embedding": [0.0]}],
            # No usage block — adapter falls back to sum(len(t) // 4 for t in texts).
        })
        result = await adapter.embed(["hello world"])  # 11 chars // 4 = 2
        assert result[0].input_tokens == 2

    @pytest.mark.asyncio
    async def test_embed_when_uninitialized_raises(self, adapter):
        with pytest.raises(BackendUnavailableError):
            await adapter.embed(["x"])

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
    async def test_load_lora_success(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.lora_load = AsyncMock(return_value=None)
        ok = await adapter.load_lora("alpha", "/srv/loras/alpha")
        assert ok is True
        adapter._client.lora_load.assert_awaited_once_with("alpha", "/srv/loras/alpha")

    @pytest.mark.asyncio
    async def test_load_lora_failure_returns_false(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.lora_load = AsyncMock(side_effect=RuntimeError("bad path"))
        ok = await adapter.load_lora("alpha", "/missing")
        assert ok is False

    @pytest.mark.asyncio
    async def test_unload_lora_success(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.lora_unload = AsyncMock(return_value=None)
        ok = await adapter.unload_lora("alpha")
        assert ok is True

    @pytest.mark.asyncio
    async def test_unload_lora_failure_returns_false(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.lora_unload = AsyncMock(side_effect=RuntimeError("not loaded"))
        ok = await adapter.unload_lora("alpha")
        assert ok is False

    @pytest.mark.asyncio
    async def test_list_models(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.models = AsyncMock(return_value=[
            {"id": "m-1"}, {"id": "m-2"},
        ])
        names = await adapter.list_models()
        assert names == ["m-1", "m-2"]

    @pytest.mark.asyncio
    async def test_switch_model_when_available(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.models = AsyncMock(return_value=[{"id": "m-1"}, {"id": "m-2"}])
        ok = await adapter.switch_model("m-2")
        assert ok is True
        assert adapter._model == "m-2"

    @pytest.mark.asyncio
    async def test_switch_model_when_missing_returns_false(self, adapter):
        adapter._initialized = True
        adapter._client = AsyncMock()
        adapter._client.models = AsyncMock(return_value=[{"id": "m-1"}])
        ok = await adapter.switch_model("m-99")
        assert ok is False
        # Model not changed.
        assert adapter._model != "m-99"


class TestVllmStreaming:
    """Real-HTTP streaming tests: no client mock anywhere in the path.

    Each test stands up an actual `ThreadingHTTPServer` on a free
    localhost port, wires the adapter to a real `VllmClient` pointing
    at it, and drives `_generate_stream` through `asyncio.to_thread` +
    the real SSE parser. That exercises every byte of
    `urllib.request.urlopen`, `for line in resp:`, the `data:` prefix
    handling, the `[DONE]` terminator, and `json.loads` on each chunk.
    Live-backend coverage against a real vLLM server is still
    `test_integration_live.py` territory (different, opt-in, gated by
    `VLLM_HOST`).
    """

    @pytest.fixture
    def adapter(self):
        a = VllmAdapter()
        a._initialized = True
        a._config = VllmConfig.from_dict({})
        a._model = "test-model"
        return a

    @staticmethod
    def _wire(adapter: VllmAdapter, url: str) -> None:
        adapter._client = VllmClient(
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
        # vLLM sometimes ends a stream with an empty-content chunk that
        # carries the finish_reason. The adapter's `elif chunk.finish_reason:`
        # branch emits a synthetic empty-text is_end_of_text token in that
        # case.
        recipe = StreamRecipe(chunks=[
            ("alpha", None),
            ("beta", None),
            ("", "stop"),
        ])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            tokens = [t async for t in gen]
        # The adapter yields a content token for each non-empty content,
        # then a synthetic end-token for the empty-content + finish_reason
        # frame. Indices are monotonic across the whole sequence.
        assert [t.text for t in tokens] == ["alpha", "beta", ""]
        assert tokens[-1].is_end_of_text is True
        assert tokens[-1].text == ""
        assert [t.index for t in tokens] == sorted(t.index for t in tokens)

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
    async def test_request_counter_increments_after_full_drain(self, adapter):
        before = adapter._requests_served
        recipe = StreamRecipe(chunks=[("ok", "stop")])
        with streaming_server(recipe) as url:
            self._wire(adapter, url)
            gen = await adapter.generate("Hi", GenerationParams(), stream=True)
            # Counter lives AFTER the `for chunk in chunks:` loop in
            # _generate_stream, so it only increments once the async
            # generator is exhausted.
            assert adapter._requests_served == before
            _ = [t async for t in gen]
        assert adapter._requests_served == before + 1


class TestVllmClientErrorMapping:
    """Drive client._handle_http_error directly against the typed-error
    contract. These tests pin the matrix's "most complete of the 7"
    claim for the vLLM client and act as a guard against regressions
    in the per-status routing.
    """

    @pytest.fixture
    def client(self):
        return VllmClient("http://127.0.0.1:65535", timeout_ms=1000, stream_timeout_ms=1000)

    def test_404_maps_to_model_not_found(self, client):
        with pytest.raises(ModelNotFoundError):
            client._handle_http_error(404, "")

    def test_429_maps_to_rate_limited(self, client):
        with pytest.raises(RateLimitedError):
            client._handle_http_error(429, "")

    def test_408_maps_to_timeout(self, client):
        with pytest.raises(AdapterTimeoutError):
            client._handle_http_error(408, "")

    def test_504_maps_to_timeout(self, client):
        with pytest.raises(AdapterTimeoutError):
            client._handle_http_error(504, "")

    def test_500_with_oom_message_maps_to_oom(self, client):
        body = json.dumps({"message": "CUDA out of memory while allocating"})
        with pytest.raises(OutOfMemoryError):
            client._handle_http_error(500, body)

    def test_400_with_context_length_message_maps_to_context_exceeded(self, client):
        body = json.dumps({"message": "This model's maximum context length is 8192 tokens"})
        with pytest.raises(ContextExceededError):
            client._handle_http_error(400, body)

    def test_generic_500_maps_to_backend_unavailable(self, client):
        with pytest.raises(BackendUnavailableError):
            client._handle_http_error(500, "internal server error")

    def test_500_with_malformed_body_falls_through_to_unavailable(self, client):
        with pytest.raises(BackendUnavailableError):
            client._handle_http_error(500, "not json at all")

    def test_400_without_known_detail_does_not_raise(self, client):
        # The handler only raises for the mapped buckets. Status 400
        # with a generic detail returns silently so the caller's
        # post-handle BackendUnavailableError fallback can kick in.
        client._handle_http_error(400, "bad request")  # no exception expected

    def test_backend_unreachable_raises_typed_error(self, client):
        # Port 65535 is closed. The request layer maps URLError to either
        # BackendUnavailableError (Linux: connection refused immediately)
        # or AdapterTimeoutError (Windows: SYN drop hits the 1s timeout).
        # Both are valid per ADAPTER-SHARED-CONTRACT §"Error Mapping" —
        # the load-bearing assertion is that a TYPED MAI error surfaces,
        # not a raw urllib exception leaking across the boundary.
        with pytest.raises((BackendUnavailableError, AdapterTimeoutError)):
            client.models()
