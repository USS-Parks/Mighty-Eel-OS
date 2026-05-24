"""Integration tests for the SGLang adapter against a real local HTTP
server (stdlib only).

These tests drive the actual `SglangClient` + `SglangAdapter` end-to-end
through `urllib.request.urlopen`, the SSE parser, the JSON unwrap, and
the typed-error mapping. They are NOT live-backend tests — no real
SGLang is needed; the test server runs on a free localhost port and
speaks the subset of the SGLang HTTP shape the adapter consumes.

J-20 (DOUGHERTY lane). Satisfies the integration mock minimums in
`docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Integration Mock Test Minimums".
"""

from __future__ import annotations

import json
import socket
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, ClassVar

import pytest

from adapters.base import (
    BackendUnavailableError,
    GenerationParams,
    HealthStatusKind,
    ModelNotFoundError,
    RateLimitedError,
    Token,
)
from adapters.sglang.adapter import SglangAdapter

# ─── fake SGLang server ────────────────────────────────────────────────────


@dataclass
class FakeRecipe:
    """Programmable response shape for one fake-server lifetime."""

    model_id: str = "fake-sglang-model"
    stream_chunks: list[tuple[str, str | None]] = field(default_factory=list)
    raw_stream_payloads: list[str] | None = None
    include_done: bool = True
    non_stream_text: str = "fake-completion"
    health_status: int = 200
    error_status: int | None = None  # if set, /v1/chat/completions returns it
    error_body: dict[str, Any] = field(default_factory=dict)


def _sse(payload: dict[str, Any]) -> bytes:
    return f"data: {json.dumps(payload)}\n\n".encode()


def _build_stream(recipe: FakeRecipe) -> bytes:
    parts: list[bytes] = []
    if recipe.raw_stream_payloads is not None:
        for raw in recipe.raw_stream_payloads:
            parts.append(f"data: {raw}\n\n".encode())
    else:
        for content, finish in recipe.stream_chunks:
            parts.append(_sse({
                "choices": [{
                    "delta": {"content": content},
                    "finish_reason": finish,
                }],
            }))
    if recipe.include_done:
        parts.append(b"data: [DONE]\n\n")
    return b"".join(parts)


class _Handler(BaseHTTPRequestHandler):
    recipe: FakeRecipe = FakeRecipe()
    hit_count: ClassVar[dict[str, int]] = {}

    def log_message(self, *_args: Any) -> None:
        return

    def _write_json(self, code: int, payload: dict[str, Any]) -> None:
        data = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        if self.path == "/health":
            if self.recipe.health_status != 200:
                self.send_error(self.recipe.health_status)
                return
            self._write_json(200, {"status": "ok"})
            return
        if self.path == "/v1/models":
            self._write_json(200, {"data": [{"id": self.recipe.model_id}]})
            return
        if self.path == "/get_model_info":
            self._write_json(200, {"model_id": self.recipe.model_id, "is_loaded": True})
            return
        self.send_error(404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            body = {}

        self.hit_count[self.path] = self.hit_count.get(self.path, 0) + 1

        if self.path == "/flush_cache":
            self._write_json(200, {"status": "ok"})
            return

        if self.path == "/v1/chat/completions":
            if self.recipe.error_status is not None:
                err_payload = (
                    json.dumps(self.recipe.error_body or {"error": "fail"}).encode()
                )
                self.send_response(self.recipe.error_status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(err_payload)))
                self.end_headers()
                self.wfile.write(err_payload)
                return
            if body.get("stream"):
                payload = _build_stream(self.recipe)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            self._write_json(200, {
                "choices": [{
                    "message": {"content": self.recipe.non_stream_text},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 3},
            })
            return

        self.send_error(404)


@contextmanager
def sglang_fake_server(recipe: FakeRecipe) -> Iterator[str]:
    """Run a fake SGLang HTTP server on a free localhost port."""
    handler_cls = type(
        "BoundHandler",
        (_Handler,),
        {"recipe": recipe, "hit_count": {}},
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


def _free_port() -> int:
    """Pick a port and immediately close, leaving it unbound. Useful for
    'backend not listening' tests — the adapter will try to connect to
    a port nothing answers on."""
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _config_for(base_url: str) -> dict[str, Any]:
    # `base_url` looks like 'http://127.0.0.1:PORT'.
    _, _, hostport = base_url.partition("://")
    host, _, port = hostport.partition(":")
    return {
        "host": host,
        "port": int(port),
        "timeout_ms": 2000,
        "stream_timeout_ms": 5000,
    }


# ─── tests ─────────────────────────────────────────────────────────────────


pytestmark = pytest.mark.integration


@pytest.mark.asyncio
async def test_initialize_against_fake_server_discovers_model() -> None:
    recipe = FakeRecipe(model_id="server-side-model")
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            handle = await adapter.initialize()
            assert handle == "server-side-model"
            assert adapter._initialized is True
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_initialize_fails_when_backend_not_listening() -> None:
    """Unbound port → BackendUnavailableError. The adapter must NOT
    leak a raw URLError or OSError across the boundary."""
    port = _free_port()
    adapter = SglangAdapter({
        "host": "127.0.0.1",
        "port": port,
        "timeout_ms": 500,
        "stream_timeout_ms": 1000,
    })
    with pytest.raises(BackendUnavailableError):
        await adapter.initialize()


@pytest.mark.asyncio
async def test_non_streaming_generate_against_fake_server() -> None:
    recipe = FakeRecipe(non_stream_text="real-wire-completion")
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            result = await adapter.generate("hello", GenerationParams())
            assert result.text == "real-wire-completion"
            assert result.tokens_generated == 3
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_streaming_generate_against_fake_server() -> None:
    recipe = FakeRecipe(
        stream_chunks=[("Once ", None), ("upon ", None), ("a time.", "stop")],
    )
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            tokens: list[Token] = []
            stream = await adapter.generate("tell me a story", GenerationParams(), stream=True)
            async for tok in stream:
                tokens.append(tok)
            assert [t.text for t in tokens] == ["Once ", "upon ", "a time."]
            assert tokens[-1].is_end_of_text is True
            # Indices are monotonically increasing starting at 0.
            assert [t.index for t in tokens] == [0, 1, 2]
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_streaming_malformed_payload_is_skipped() -> None:
    """SGLang occasionally emits malformed `data:` lines under load.
    The client must skip them rather than blow up the stream."""
    recipe = FakeRecipe(
        raw_stream_payloads=[
            "not-json-at-all",  # malformed → silently skipped
            json.dumps({
                "choices": [{
                    "delta": {"content": "survived"},
                    "finish_reason": None,
                }],
            }),
            json.dumps({
                "choices": [{
                    "delta": {"content": ""},
                    "finish_reason": "stop",
                }],
            }),
        ],
    )
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            tokens: list[Token] = [
                t
                async for t in await adapter.generate(
                    "robust?", GenerationParams(), stream=True,
                )
            ]
            assert any(t.text == "survived" for t in tokens)
            assert tokens[-1].is_end_of_text is True
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_client_reused_across_two_real_requests() -> None:
    """Two consecutive generates should use the same `SglangClient`
    instance — and the server should observe two POSTs to the chat
    completions endpoint, not four (no per-request reconnect dance)."""
    recipe = FakeRecipe(non_stream_text="x")
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            client_id_before = id(adapter._client)
            await adapter.generate("a", GenerationParams())
            await adapter.generate("b", GenerationParams())
            assert id(adapter._client) == client_id_before
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_shutdown_releases_client_after_real_session() -> None:
    recipe = FakeRecipe()
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        await adapter.initialize()
        await adapter.generate("warmup", GenerationParams())
        await adapter.shutdown()
        assert adapter._client is None
        assert adapter._initialized is False


@pytest.mark.asyncio
async def test_backend_503_maps_to_backend_unavailable() -> None:
    recipe = FakeRecipe(error_status=503, error_body={"error": "service down"})
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            with pytest.raises(BackendUnavailableError):
                await adapter.generate("hi", GenerationParams())
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_backend_429_maps_to_rate_limited() -> None:
    recipe = FakeRecipe(error_status=429, error_body={"error": "slow down"})
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            with pytest.raises(RateLimitedError):
                await adapter.generate("hi", GenerationParams())
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_backend_404_maps_to_model_not_found() -> None:
    recipe = FakeRecipe(error_status=404, error_body={"error": "no such model"})
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            with pytest.raises(ModelNotFoundError):
                await adapter.generate("hi", GenerationParams())
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_health_check_against_real_server_reports_healthy() -> None:
    recipe = FakeRecipe()
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            status = await adapter.health_check()
            assert status.kind == HealthStatusKind.HEALTHY
        finally:
            await adapter.shutdown()


@pytest.mark.asyncio
async def test_flush_cache_against_real_server() -> None:
    recipe = FakeRecipe()
    with sglang_fake_server(recipe) as url:
        adapter = SglangAdapter(_config_for(url))
        try:
            await adapter.initialize()
            assert await adapter.flush_cache() is True
        finally:
            await adapter.shutdown()
