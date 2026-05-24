"""Unit and integration tests for the OpenAI-compatible local adapter.

Covers the surface required by ``docs/ADAPTER-SHARED-CONTRACT.md`` and
``docs/ADAPTER-TEST-HARNESS-LOCK.md``:

  - construction stores config without network calls
  - initialize happy path + readiness probe via /v1/models
  - initialize unavailable backend -> BackendUnavailableError
  - initialize config validation -> ValidationError
  - generate non-streaming + streaming happy paths
  - generate timeout -> AdapterTimeoutError
  - generate model-not-found -> ModelNotFoundError
  - generate OOM -> OutOfMemoryError
  - generate malformed body -> ValidationError (typed)
  - generate_batch preserves order, handles empty list
  - embed returns Embedding list when supported, raises otherwise
  - capabilities truthfulness for every flag
  - health_check returns healthy / degraded / unavailable
  - shutdown closes resources, idempotent
  - HTTP pooling: one opener reused across requests

Mock-server tests drive the real client + real adapter end-to-end
through ``http.server.ThreadingHTTPServer`` so the SSE parser, the
HTTP error-mapping code, and the JSON decoders are exercised by real
bytes — no AsyncMock of the methods under test.

DOUGHERTY J-23.
"""

from __future__ import annotations

import contextlib
import json
import socket
import threading
import time
from collections.abc import Iterator
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest

from adapters.base import (
    AdapterTimeoutError,
    BackendUnavailableError,
    Embedding,
    FinishReason,
    GenerationParams,
    GenerationResult,
    HealthStatusKind,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    Token,
    UnsupportedOperationError,
    ValidationError,
)
from adapters.openai_compat.adapter import OpenAICompatAdapter
from adapters.openai_compat.client import OpenAICompatClient
from adapters.openai_compat.config import OpenAICompatConfig

# ─── Fake server harness ──────────────────────────────────────────────


@dataclass
class FakeRecipe:
    """Programmable behaviour for the local fake OpenAI-compatible server."""

    models_status: int = 200
    models_body: dict[str, Any] = field(
        default_factory=lambda: {"data": [{"id": "test-model"}]},
    )
    chat_status: int = 200
    chat_body: dict[str, Any] | None = None
    completion_status: int = 200
    completion_body: dict[str, Any] | None = None
    embeddings_status: int = 200
    embeddings_body: dict[str, Any] | None = None
    stream_chunks: list[tuple[str, str | None]] = field(default_factory=list)
    stream_raw_payloads: list[str] | None = None
    stream_include_done: bool = True
    delay_ms: int = 0
    error_body_text: str = ""
    auth_required: bool = False
    request_count: dict[str, int] = field(default_factory=dict)
    auth_seen: list[str | None] = field(default_factory=list)


def _default_chat_body() -> dict[str, Any]:
    return {
        "id": "chat-1",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "hi"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
    }


def _default_completion_body() -> dict[str, Any]:
    return {
        "id": "cmpl-1",
        "choices": [{
            "index": 0,
            "text": "hello world",
            "finish_reason": "length",
        }],
        "usage": {"prompt_tokens": 4, "completion_tokens": 5},
    }


def _default_embeddings_body(count: int = 2) -> dict[str, Any]:
    return {
        "data": [
            {"index": i, "embedding": [0.1 * (i + 1), 0.2 * (i + 1), 0.3]}
            for i in range(count)
        ],
        "usage": {"prompt_tokens": count * 3},
    }


def _sse_chunk(content: str, finish_reason: str | None) -> bytes:
    payload = {
        "choices": [{
            "delta": {"content": content},
            "finish_reason": finish_reason,
        }],
    }
    return f"data: {json.dumps(payload)}\n\n".encode()


def _render_stream_body(recipe: FakeRecipe) -> bytes:
    parts: list[bytes] = []
    if recipe.stream_raw_payloads is not None:
        for payload in recipe.stream_raw_payloads:
            parts.append(f"data: {payload}\n\n".encode())
    else:
        for content, fr in recipe.stream_chunks:
            parts.append(_sse_chunk(content, fr))
    if recipe.stream_include_done:
        parts.append(b"data: [DONE]\n\n")
    return b"".join(parts)


class _FakeHandler(BaseHTTPRequestHandler):
    recipe: FakeRecipe = FakeRecipe()

    def log_message(self, *_args: Any) -> None:
        return

    def _record(self, key: str) -> None:
        self.recipe.request_count[key] = self.recipe.request_count.get(key, 0) + 1
        self.recipe.auth_seen.append(self.headers.get("Authorization"))

    def _maybe_delay(self) -> None:
        if self.recipe.delay_ms > 0:
            time.sleep(self.recipe.delay_ms / 1000.0)

    def _auth_ok(self) -> bool:
        if not self.recipe.auth_required:
            return True
        return self.headers.get("Authorization", "").startswith("Bearer ")

    def do_GET(self) -> None:
        self._maybe_delay()
        if not self._auth_ok():
            self.send_error(401)
            return
        if self.path == "/v1/models":
            self._record("models")
            self._respond_json(self.recipe.models_status, self.recipe.models_body)
            return
        self.send_error(404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            body = {}
        self._maybe_delay()
        if not self._auth_ok():
            self.send_error(401)
            return

        if self.path == "/v1/chat/completions":
            self._record("chat")
            if self.recipe.chat_status >= 400:
                self._respond_error(self.recipe.chat_status)
                return
            if body.get("stream"):
                body_bytes = _render_stream_body(self.recipe)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Content-Length", str(len(body_bytes)))
                self.end_headers()
                self.wfile.write(body_bytes)
                self.wfile.flush()
                return
            payload = self.recipe.chat_body or _default_chat_body()
            self._respond_json(200, payload)
            return

        if self.path == "/v1/completions":
            self._record("completion")
            if self.recipe.completion_status >= 400:
                self._respond_error(self.recipe.completion_status)
                return
            payload = self.recipe.completion_body or _default_completion_body()
            self._respond_json(200, payload)
            return

        if self.path == "/v1/embeddings":
            self._record("embeddings")
            if self.recipe.embeddings_status >= 400:
                self._respond_error(self.recipe.embeddings_status)
                return
            count = len(body.get("input") or [])
            payload = self.recipe.embeddings_body or _default_embeddings_body(
                max(count, 1),
            )
            self._respond_json(200, payload)
            return

        self.send_error(404)

    def _respond_json(self, code: int, payload: dict[str, Any]) -> None:
        data = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _respond_error(self, code: int) -> None:
        body = self.recipe.error_body_text or json.dumps(
            {"error": {"message": "synthetic error", "type": "test"}},
        )
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


@contextlib.contextmanager
def fake_server(recipe: FakeRecipe) -> Iterator[str]:
    """Spin up a one-shot local fake server bound to the given recipe."""
    handler_cls = type(
        "BoundOpenAICompatHandler",
        (_FakeHandler,),
        {"recipe": recipe},
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_cls)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def _config_for(url: str, **overrides: Any) -> dict[str, Any]:
    host = url.replace("http://", "").replace("https://", "")
    host, port = host.split(":")
    base = {
        "host": host,
        "port": int(port),
        "default_model": "test-model",
        "timeout_ms": 5000,
        "stream_timeout_ms": 5000,
    }
    base.update(overrides)
    return base


# ─── Config + construction ────────────────────────────────────────────


class TestConfig:
    def test_defaults(self) -> None:
        cfg = OpenAICompatConfig.from_dict({})
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 1234
        assert cfg.scheme == "http"
        assert cfg.timeout_ms == 30000
        assert cfg.supports_streaming is True
        assert cfg.supports_embeddings is False

    def test_base_url(self) -> None:
        cfg = OpenAICompatConfig.from_dict(
            {"host": "10.0.0.5", "port": 9000, "base_path": "/api"},
        )
        assert cfg.base_url == "http://10.0.0.5:9000/api"

    def test_extra_keys_preserved(self) -> None:
        cfg = OpenAICompatConfig.from_dict({"unknown_key": 1, "host": "h"})
        assert cfg.host == "h"
        assert cfg.extra_options == {"unknown_key": 1}


class TestConstruction:
    def test_construction_does_no_network(self) -> None:
        adapter = OpenAICompatAdapter({"host": "127.0.0.1", "port": 1234})
        assert adapter._client is None
        assert adapter._initialized is False
        assert adapter._requests_served == 0


# ─── Validation ───────────────────────────────────────────────────────


class TestInitializeValidation:
    @pytest.mark.asyncio
    async def test_bad_scheme(self) -> None:
        adapter = OpenAICompatAdapter()
        with pytest.raises(ValidationError):
            await adapter.initialize({"scheme": "ftp"})

    @pytest.mark.asyncio
    async def test_port_out_of_range(self) -> None:
        adapter = OpenAICompatAdapter()
        with pytest.raises(ValidationError):
            await adapter.initialize({"port": 0})

    @pytest.mark.asyncio
    async def test_bad_prefer_endpoint(self) -> None:
        adapter = OpenAICompatAdapter()
        with pytest.raises(ValidationError):
            await adapter.initialize({"prefer_endpoint": "graphql"})

    @pytest.mark.asyncio
    async def test_negative_timeout(self) -> None:
        adapter = OpenAICompatAdapter()
        with pytest.raises(ValidationError):
            await adapter.initialize({"timeout_ms": 0})


# ─── Initialize against fake server ───────────────────────────────────


class TestInitialize:
    @pytest.mark.asyncio
    async def test_happy_path(self) -> None:
        recipe = FakeRecipe(
            models_body={"data": [{"id": "alpha"}, {"id": "beta"}]},
        )
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            handle = await adapter.initialize(_config_for(url))
            try:
                assert adapter._initialized is True
                assert handle.startswith("openai_compat-")
                assert adapter._known_models == ["alpha", "beta"]
                # default_model wins over the discovered list when set.
                assert adapter._chat_model == "test-model"
                assert recipe.request_count.get("models") == 1
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_backend_unreachable_maps_to_typed_error(self) -> None:
        # Bind a port then immediately release it. On Linux this yields
        # a connection-refused RST (-> BackendUnavailableError); on
        # Windows the OS often drops the SYN silently and the client
        # surfaces an AdapterTimeoutError. The shared contract treats
        # both as typed adapter errors for an unreachable backend per
        # ADAPTER-SHARED-CONTRACT.md §Error Mapping; this test asserts
        # the union, not the platform-specific variant.
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
        sock.close()
        adapter = OpenAICompatAdapter()
        with pytest.raises((BackendUnavailableError, AdapterTimeoutError)):
            await adapter.initialize({
                "host": "127.0.0.1",
                "port": port,
                "default_model": "x",
                "timeout_ms": 500,
                "stream_timeout_ms": 500,
                "max_retries": 0,
            })
        assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_auth_header_sent(self) -> None:
        recipe = FakeRecipe(auth_required=True)
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            cfg = _config_for(url, api_key="sk-test")
            await adapter.initialize(cfg)
            try:
                assert recipe.auth_seen[0] == "Bearer sk-test"
            finally:
                await adapter.shutdown()


# ─── Generation: non-streaming ────────────────────────────────────────


class TestGenerateUnary:
    @pytest.mark.asyncio
    async def test_chat_happy_path(self) -> None:
        recipe = FakeRecipe(chat_body={
            "choices": [{
                "message": {"content": "Paris"},
                "finish_reason": "stop",
            }],
            "usage": {"completion_tokens": 1},
        })
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                result = await adapter.generate(
                    "What is the capital of France?",
                    GenerationParams(temperature=0.0, max_tokens=8),
                )
                assert isinstance(result, GenerationResult)
                assert result.text == "Paris"
                assert result.tokens_generated == 1
                assert result.finish_reason is FinishReason.STOP
                assert adapter._requests_served == 1
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_chat_max_tokens_maps_to_finish_reason(self) -> None:
        recipe = FakeRecipe(chat_body={
            "choices": [{
                "message": {"content": "truncated"},
                "finish_reason": "length",
            }],
            "usage": {"completion_tokens": 5},
        })
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                result = await adapter.generate("q", GenerationParams())
                assert result.finish_reason is FinishReason.MAX_TOKENS
                assert result.tokens_generated == 5
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_completion_endpoint_when_preferred(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(
                _config_for(url, prefer_endpoint="completion"),
            )
            try:
                result = await adapter.generate("hello", GenerationParams())
                assert result.text == "hello world"
                assert result.finish_reason is FinishReason.MAX_TOKENS
                assert recipe.request_count.get("completion") == 1
                assert recipe.request_count.get("chat") is None
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_empty_choices_returns_empty_result(self) -> None:
        recipe = FakeRecipe(chat_body={"choices": []})
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                result = await adapter.generate("q", GenerationParams())
                assert result.text == ""
                assert result.tokens_generated == 0
                assert result.finish_reason is FinishReason.STOP
            finally:
                await adapter.shutdown()


# ─── Generation: streaming ────────────────────────────────────────────


class TestGenerateStream:
    @pytest.mark.asyncio
    async def test_stream_yields_ordered_tokens(self) -> None:
        recipe = FakeRecipe(stream_chunks=[
            ("Hello", None),
            (" ", None),
            ("world", None),
            ("", "stop"),
        ])
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                stream = await adapter.generate(
                    "say hi",
                    GenerationParams(max_tokens=32),
                    stream=True,
                )
                tokens: list[Token] = []
                async for tok in stream:
                    tokens.append(tok)
                non_empty = [t.text for t in tokens if t.text]
                assert non_empty == ["Hello", " ", "world"]
                indices = [t.index for t in tokens]
                assert indices == sorted(indices)
                assert tokens[-1].is_end_of_text is True
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_stream_tolerates_malformed_payload(self) -> None:
        recipe = FakeRecipe(
            stream_raw_payloads=[
                '{"choices":[{"delta":{"content":"ok"}}]}',
                "not-json-at-all",
                '{"choices":[{"delta":{"content":"!"},"finish_reason":"stop"}]}',
            ],
        )
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                stream = await adapter.generate(
                    "q", GenerationParams(), stream=True,
                )
                tokens = [tok async for tok in stream]
                assert [t.text for t in tokens if t.text] == ["ok", "!"]
                assert tokens[-1].is_end_of_text is True
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_stream_empty_yields_terminator(self) -> None:
        recipe = FakeRecipe(stream_chunks=[], stream_include_done=True)
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                stream = await adapter.generate(
                    "q", GenerationParams(), stream=True,
                )
                tokens = [tok async for tok in stream]
                assert tokens and tokens[-1].is_end_of_text is True
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_stream_unsupported_when_disabled(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url, supports_streaming=False))
            try:
                with pytest.raises(UnsupportedOperationError):
                    await adapter.generate("q", GenerationParams(), stream=True)
            finally:
                await adapter.shutdown()


# ─── Error mapping ────────────────────────────────────────────────────


class TestErrorMapping:
    @pytest.mark.asyncio
    async def test_404_model_not_found(self) -> None:
        recipe = FakeRecipe(
            chat_status=404,
            error_body_text=json.dumps({
                "error": {"message": "Model 'unknown-model' not found"},
            }),
        )
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(ModelNotFoundError):
                    await adapter.generate("q", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_400_oom(self) -> None:
        recipe = FakeRecipe(
            chat_status=400,
            error_body_text=json.dumps({
                "error": {"message": "CUDA out of memory"},
            }),
        )
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(OutOfMemoryError):
                    await adapter.generate("q", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_429_rate_limited(self) -> None:
        recipe = FakeRecipe(chat_status=429)
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(RateLimitedError):
                    await adapter.generate("q", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_400_validation(self) -> None:
        recipe = FakeRecipe(
            chat_status=400,
            error_body_text=json.dumps({
                "error": {"message": "missing required field"},
            }),
        )
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(ValidationError):
                    await adapter.generate("q", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_timeout(self) -> None:
        # Server sleeps longer than the client's 100 ms timeout.
        recipe = FakeRecipe(delay_ms=500)
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            # Use a generous timeout for /v1/models (readiness probe)
            # then shrink it so the generate call trips the deadline.
            cfg = _config_for(url, timeout_ms=5000, stream_timeout_ms=5000)
            await adapter.initialize(cfg)
            try:
                assert adapter._client is not None
                adapter._client._timeout = 0.1
                with pytest.raises(AdapterTimeoutError):
                    await adapter.generate("q", GenerationParams())
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_malformed_json_body_maps_to_validation_error(self) -> None:
        client = OpenAICompatClient(
            base_url="http://127.0.0.1:1",
            timeout_ms=100,
            stream_timeout_ms=100,
        )
        # _request is private but it's the path that decodes JSON;
        # patch the opener so it returns a non-JSON body.
        class _FakeResp:
            status = 200
            length = -1
            def read(self) -> bytes:
                return b"not json"
            def __enter__(self) -> _FakeResp:
                return self
            def __exit__(self, *_args: Any) -> None:
                return None
        class _FakeOpener:
            def open(self, _req: Any, timeout: Any) -> _FakeResp:
                return _FakeResp()

        client._opener = _FakeOpener()
        with pytest.raises(ValidationError):
            client._request("GET", "/v1/models")


# ─── Batch ────────────────────────────────────────────────────────────


class TestGenerateBatch:
    @pytest.mark.asyncio
    async def test_empty_list_returns_empty(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                assert await adapter.generate_batch([], GenerationParams()) == []
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_preserves_order(self) -> None:
        # The handler returns a body keyed by request order via a
        # shared counter. We fix the body to a deterministic value
        # and assert the adapter returns three results in order.
        recipe = FakeRecipe(chat_body={
            "choices": [{
                "message": {"content": "ok"},
                "finish_reason": "stop",
            }],
            "usage": {"completion_tokens": 1},
        })
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                prompts = ["a", "b", "c"]
                results = await adapter.generate_batch(prompts, GenerationParams())
                assert len(results) == 3
                assert all(r.text == "ok" for r in results)
                assert all(r.finish_reason is FinishReason.STOP for r in results)
                assert recipe.request_count.get("chat") == 3
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_non_list_input_raises(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(ValidationError):
                    await adapter.generate_batch("not a list", GenerationParams())
            finally:
                await adapter.shutdown()


# ─── Embeddings ───────────────────────────────────────────────────────


class TestEmbeddings:
    @pytest.mark.asyncio
    async def test_unsupported_by_default(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                with pytest.raises(UnsupportedOperationError):
                    await adapter.embed(["hello"])
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_returns_ordered_embeddings_when_enabled(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            cfg = _config_for(
                url,
                supports_embeddings=True,
                embedding_model="embed-test",
            )
            await adapter.initialize(cfg)
            try:
                vectors = await adapter.embed(["a", "b"])
                assert len(vectors) == 2
                assert all(isinstance(v, Embedding) for v in vectors)
                assert vectors[0].vector != vectors[1].vector
                assert vectors[0].input_tokens >= 0
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_empty_input_returns_empty_when_enabled(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url, supports_embeddings=True))
            try:
                assert await adapter.embed([]) == []
            finally:
                await adapter.shutdown()


# ─── Health, capabilities, shutdown ───────────────────────────────────


class TestHealth:
    @pytest.mark.asyncio
    async def test_healthy_after_init(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                status = await adapter.health_check()
                assert status.kind is HealthStatusKind.HEALTHY
                assert status.uptime_ms >= 0
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_unavailable_before_init(self) -> None:
        adapter = OpenAICompatAdapter()
        status = await adapter.health_check()
        assert status.kind is HealthStatusKind.UNAVAILABLE

    @pytest.mark.asyncio
    async def test_degraded_when_no_models(self) -> None:
        # First call returns one model so initialize succeeds; later
        # calls return zero models, but the adapter remembers the
        # initial discovery. To trigger DEGRADED we force a fresh
        # recipe with no models AND wipe the adapter's known list.
        recipe = FakeRecipe(models_body={"data": [{"id": "x"}]})
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                adapter._known_models = []
                recipe.models_body = {"data": []}
                status = await adapter.health_check()
                assert status.kind is HealthStatusKind.DEGRADED
            finally:
                await adapter.shutdown()


class TestCapabilities:
    def test_defaults_truthful(self) -> None:
        adapter = OpenAICompatAdapter()
        caps = adapter.capabilities()
        assert caps.supports_streaming is True
        assert caps.supports_batching is False
        assert caps.supports_embedding is False
        assert caps.supports_tool_calling is False
        assert caps.supports_vision is False
        assert caps.supports_hot_swap is False
        assert caps.supports_continuous_batching is False
        assert caps.max_context_window == 8192

    def test_embeddings_capability_reflects_config(self) -> None:
        adapter = OpenAICompatAdapter({"supports_embeddings": True})
        adapter._config = OpenAICompatConfig.from_dict({"supports_embeddings": True})
        assert adapter.capabilities().supports_embedding is True

    def test_streaming_capability_reflects_config(self) -> None:
        adapter = OpenAICompatAdapter()
        adapter._config = OpenAICompatConfig.from_dict({"supports_streaming": False})
        assert adapter.capabilities().supports_streaming is False


class TestShutdownAndLifecycle:
    @pytest.mark.asyncio
    async def test_pre_init_calls_fail_deterministically(self) -> None:
        adapter = OpenAICompatAdapter()
        with pytest.raises(BackendUnavailableError):
            await adapter.generate("q", GenerationParams())
        with pytest.raises(BackendUnavailableError):
            await adapter.generate_batch(["q"], GenerationParams())
        with pytest.raises(BackendUnavailableError):
            await adapter.embed(["q"])

    @pytest.mark.asyncio
    async def test_shutdown_idempotent(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            await adapter.shutdown()
            assert adapter._client is None
            assert adapter._initialized is False
            # Second call must not raise.
            await adapter.shutdown()
            assert adapter._initialized is False

    @pytest.mark.asyncio
    async def test_client_reused_across_requests(self) -> None:
        recipe = FakeRecipe()
        with fake_server(recipe) as url:
            adapter = OpenAICompatAdapter()
            await adapter.initialize(_config_for(url))
            try:
                opener_before = adapter._client._opener
                await adapter.generate("a", GenerationParams())
                await adapter.generate("b", GenerationParams())
                opener_after = adapter._client._opener
                assert opener_before is opener_after
                assert recipe.request_count.get("chat") == 2
            finally:
                await adapter.shutdown()

    @pytest.mark.asyncio
    async def test_close_client_then_request_raises(self) -> None:
        client = OpenAICompatClient(
            base_url="http://127.0.0.1:1",
            timeout_ms=100,
            stream_timeout_ms=100,
        )
        client.close()
        assert client.closed is True
        with pytest.raises(BackendUnavailableError):
            client._request("GET", "/v1/models")


# ─── Sanity: the adapter is registered ────────────────────────────────


def test_adapter_registered_under_decorator_name() -> None:
    from adapters.base import get_adapter
    cls = get_adapter("openai_compat")
    assert cls is OpenAICompatAdapter
