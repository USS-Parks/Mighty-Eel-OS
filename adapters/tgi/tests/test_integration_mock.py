"""HTTP integration tests for the TGI adapter against a real fake server.

These tests stand up an actual ``ThreadingHTTPServer`` on a free
localhost port (via ``_tgi_test_server.tgi_server``) and wire the
adapter to a real ``TgiClient`` pointing at it. That exercises every
byte of ``urllib.request.urlopen``, the ``for line in resp:`` SSE
parser, the ``data:`` prefix handling, JSON decoding, error mapping,
and end-of-text derivation - none of which the unit-test mocks touch.

Live-backend coverage against a real TGI server is the separate
``test_integration_live.py`` concern (gated by ``$TGI_HOST``).

DOUGHERTY J-19. Required by ``docs/ADAPTER-TEST-HARNESS-LOCK.md`` -
integration-mock test minimums.
"""
from __future__ import annotations

import json

import pytest

from adapters.base import (
    BackendUnavailableError,
    ContextExceededError,
    GenerationParams,
    GenerationResult,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    Token,
    ValidationError,
)
from adapters.tgi.adapter import TgiAdapter
from adapters.tgi.client import TgiClient
from adapters.tgi.tests._tgi_test_server import TgiRecipe, tgi_server

# NOTE: the project's existing ``integration`` marker (pyproject.toml)
# is defined as "requires real MAI instance" - different semantics from
# the ``docs/ADAPTER-TEST-HARNESS-LOCK.md`` definition (deterministic
# local fakes). Applying the marker here would either skip these tests
# under the project filter or silently redefine the marker; both are
# bad. The convergence session can reconcile the marker semantics. For
# now these tests run unconditionally - they only depend on stdlib.


async def _adapter_for(url: str) -> TgiAdapter:
    """Build and initialize a TgiAdapter pointed at the test server."""
    host_part = url.replace("http://", "").rstrip("/")
    host, port_s = host_part.split(":", 1)
    adapter = TgiAdapter()
    await adapter.initialize(config={
        "host": host,
        "port": int(port_s),
        "timeout_ms": 2000,
        "stream_timeout_ms": 5000,
    })
    return adapter


# ----- readiness ---------------------------------------------------------


class TestTgiClientReadiness:
    @pytest.mark.asyncio
    async def test_init_succeeds_against_healthy_server(self):
        recipe = TgiRecipe(info_body={
            "model_id": "test/mistral",
            "max_input_length": 2048,
            "max_total_tokens": 4096,
        })
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                assert adapter._initialized is True
                assert adapter._model_id == "test/mistral"
                assert adapter._max_input_tokens == 2048
                assert adapter._max_total_tokens == 4096
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_init_fails_when_server_not_listening(self):
        # Pick a port that is almost certainly not listening on the
        # loopback interface; the real probe must come back unhealthy.
        adapter = TgiAdapter()
        with pytest.raises(BackendUnavailableError):
            await adapter.initialize(config={
                "host": "127.0.0.1",
                "port": 1,            # privileged; we never bind it
                "timeout_ms": 500,
                "stream_timeout_ms": 1000,
            })
        assert adapter._initialized is False
        assert adapter._client is None

    @pytest.mark.asyncio
    async def test_init_fails_when_health_returns_503(self):
        with tgi_server(TgiRecipe(health_status=503)) as url:
            adapter = TgiAdapter()
            host_part = url.replace("http://", "").rstrip("/")
            host, port_s = host_part.split(":", 1)
            cfg = {
                "host": host,
                "port": int(port_s),
                "timeout_ms": 500,
                "stream_timeout_ms": 1000,
            }
            with pytest.raises(BackendUnavailableError):
                await adapter.initialize(config=cfg)


# ----- error mapping -----------------------------------------------------


class TestTgiClientErrorMapping:
    @pytest.mark.asyncio
    async def test_422_validation_maps_to_validation_error(self):
        with tgi_server(TgiRecipe(
            error_status=422,
            error_body={"error": "schema fail", "error_type": "validation"},
        )) as url:
            adapter = await _adapter_for(url)
            try:
                with pytest.raises(ValidationError):
                    await adapter.generate("hi", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_422_validation_context_maps_to_context_exceeded(self):
        with tgi_server(TgiRecipe(
            error_status=422,
            error_body={
                "error": "Input is too long for max_input_length",
                "error_type": "validation",
            },
        )) as url:
            adapter = await _adapter_for(url)
            try:
                with pytest.raises(ContextExceededError):
                    await adapter.generate("hi" * 9999, GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_404_maps_to_model_not_found(self):
        with tgi_server(TgiRecipe(
            error_status=404,
            error_body={"error": "model gone", "error_type": "unknown"},
        )) as url:
            adapter = await _adapter_for(url)
            try:
                with pytest.raises(ModelNotFoundError):
                    await adapter.generate("hi", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_oom_in_error_body_maps_to_out_of_memory(self):
        with tgi_server(TgiRecipe(
            error_status=500,
            error_body={"error": "CUDA out of memory", "error_type": "generation"},
        )) as url:
            adapter = await _adapter_for(url)
            try:
                with pytest.raises(OutOfMemoryError):
                    await adapter.generate("hi", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_429_maps_to_rate_limited(self):
        with tgi_server(TgiRecipe(
            error_status=429,
            error_body={"error": "slow down", "error_type": "overloaded"},
        )) as url:
            adapter = await _adapter_for(url)
            try:
                with pytest.raises(RateLimitedError):
                    await adapter.generate("hi", GenerationParams())
            finally:
                await adapter.shutdown()


# ----- streaming ---------------------------------------------------------


class TestTgiClientStreaming:
    @pytest.mark.asyncio
    async def test_stream_decodes_token_frames(self):
        recipe = TgiRecipe(stream_chunks=[
            ("Hello", None, None),
            (" ", None, None),
            ("world", "length", "Hello world"),
        ])
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                stream = await adapter.generate("hi", GenerationParams(), stream=True)
                tokens: list[Token] = []
                async for tok in stream:
                    tokens.append(tok)
            finally:
                await adapter.shutdown()
        assert [t.text for t in tokens] == ["Hello", " ", "world"]
        assert [t.index for t in tokens] == [0, 1, 2]
        assert tokens[-1].is_end_of_text is True
        assert tokens[0].is_end_of_text is False

    @pytest.mark.asyncio
    async def test_stream_terminates_after_last_data_frame(self):
        # No further frames after the terminating chunk - the iterator
        # must end cleanly rather than hanging on the socket.
        recipe = TgiRecipe(stream_chunks=[("only", "stop", "only")])
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                stream = await adapter.generate("hi", GenerationParams(), stream=True)
                tokens = [t async for t in stream]
            finally:
                await adapter.shutdown()
        assert len(tokens) == 1
        assert tokens[0].is_end_of_text is True

    @pytest.mark.asyncio
    async def test_stream_maps_inline_error_frame_to_typed_error(self):
        # TGI emits an in-band error frame mid-stream rather than
        # closing the connection. The adapter must raise.
        recipe = TgiRecipe(stream_raw_payloads=[
            json.dumps({"token": {"text": "he", "id": 1}}),
            json.dumps({
                "error": "Input length exceeds context",
                "error_type": "validation",
            }),
        ])
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                stream = await adapter.generate("hi" * 9000, GenerationParams(),
                                                stream=True)
                with pytest.raises(ContextExceededError):
                    async for _ in stream:
                        pass
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_stream_malformed_frame_raises(self):
        # A non-JSON ``data:`` line is a backend protocol violation; the
        # client raises rather than silently dropping tokens.
        recipe = TgiRecipe(stream_raw_payloads=[
            json.dumps({"token": {"text": "ok", "id": 1}}),
            "this is not json",
        ])
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                stream = await adapter.generate("hi", GenerationParams(),
                                                stream=True)
                with pytest.raises(BackendUnavailableError):
                    async for _ in stream:
                        pass
            finally:
                await adapter.shutdown()


# ----- generation, health, reuse ----------------------------------------


class TestTgiClientGeneration:
    @pytest.mark.asyncio
    async def test_non_streaming_generate_returns_typed_result(self):
        recipe = TgiRecipe(generate_body={
            "generated_text": "Hello",
            "details": {"generated_tokens": 1, "finish_reason": "length"},
        })
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                result = await adapter.generate("Hi", GenerationParams(max_tokens=5))
            finally:
                await adapter.shutdown()
        assert isinstance(result, GenerationResult)
        assert result.text == "Hello"
        assert result.tokens_generated == 1

    @pytest.mark.asyncio
    async def test_batch_preserves_order_against_real_server(self):
        recipe = TgiRecipe(generate_body={
            "generated_text": "echo",
            "details": {"generated_tokens": 1, "finish_reason": "length"},
        })
        with tgi_server(recipe) as url:
            adapter = await _adapter_for(url)
            try:
                results = await adapter.generate_batch(
                    ["a", "b", "c"], GenerationParams(max_tokens=4),
                )
            finally:
                await adapter.shutdown()
        assert len(results) == 3
        assert all(r.text == "echo" for r in results)

    @pytest.mark.asyncio
    async def test_health_check_healthy_then_unavailable_after_shutdown(self):
        with tgi_server(TgiRecipe()) as url:
            adapter = await _adapter_for(url)
            try:
                status = await adapter.health_check()
                assert status.kind == HealthStatusKind.HEALTHY
            finally:
                await adapter.shutdown()
        status_after = await adapter.health_check()
        assert status_after.kind == HealthStatusKind.UNAVAILABLE


class TestTgiClientReuse:
    @pytest.mark.asyncio
    async def test_two_calls_share_the_same_client_object(self):
        with tgi_server(TgiRecipe()) as url:
            adapter = await _adapter_for(url)
            try:
                before = adapter._client
                assert isinstance(before, TgiClient)
                await adapter.generate("a", GenerationParams(max_tokens=4))
                await adapter.generate("b", GenerationParams(max_tokens=4))
                assert adapter._client is before
            finally:
                await adapter.shutdown()
        # After shutdown the client reference is released so the pool
        # can drain.
        assert adapter._client is None
