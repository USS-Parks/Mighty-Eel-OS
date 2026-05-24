"""Real HTTP/SSE test server for adapter streaming tests.

Spins up `http.server.ThreadingHTTPServer` on a free localhost port in a
daemon thread, speaks OpenAI-compatible Server-Sent Events on
`POST /v1/chat/completions` with `stream=true`, and serves the side
endpoints (`/health`, `/props`, `/v1/models`, non-streaming chat) so the
real `LlamaCppClient` and `ExLlamaV2Client` can talk to it end-to-end.

This file exists because mock-based streaming tests were too shallow:
they bypassed every byte of the client's `urllib.request.urlopen`
plumbing, the `for line in resp:` SSE parsing, the `"data: "` prefix
handling, the `[DONE]` terminator, and the malformed-line resilience.
Driving the real client + real adapter against this server exercises
all of that without requiring a live llama-server / TabbyAPI install
(those still run under `@pytest.mark.live_backend` from J-06 / J-07 /
J-21 — different concern, different cost).

Stdlib only. No new dependencies. Air-gap-policy compliant.

J-09 (DOUGHERTY lane) addendum.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


@dataclass
class StreamRecipe:
    """Programmable response shape for one test server lifetime.

    Attributes:
        chunks: list of (delta_content, finish_reason) pairs. The handler
            renders each as `data: {"choices":[{"delta":{"content":...},
            "finish_reason":...}]}\\n\\n`. `finish_reason=None` means a
            mid-stream chunk; non-None signals end-of-text in both the
            llama.cpp and ExLlamaV2 chunk dataclasses.
        raw_data_payloads: optional override. When set, the handler
            writes one `data: <payload>\\n\\n` line per entry verbatim
            (used for the malformed-JSON resilience test). Wins over
            `chunks`.
        include_done: whether to terminate the stream with `data:
            [DONE]\\n\\n` (the canonical SSE terminator). Default True.
        non_stream_body: response body for `stream=false` chat completions.
        health_body: body for `GET /health` (llama-server shape).
        models_body: body for `GET /v1/models` (TabbyAPI shape).
        props_body: body for `GET /props` (llama-server shape).
    """

    chunks: list[tuple[str, str | None]] = field(default_factory=list)
    raw_data_payloads: list[str] | None = None
    include_done: bool = True
    non_stream_body: dict[str, Any] | None = None
    health_body: dict[str, Any] | None = None
    models_body: dict[str, Any] | None = None
    props_body: dict[str, Any] | None = None


def _sse_for_chunk(content: str, finish_reason: str | None) -> bytes:
    """Render one (content, finish_reason) pair as an SSE event."""
    payload = {
        "choices": [{
            "delta": {"content": content},
            "finish_reason": finish_reason,
        }],
    }
    return f"data: {json.dumps(payload)}\n\n".encode()


def _build_stream_body(recipe: StreamRecipe) -> bytes:
    """Pre-render the full SSE body so Content-Length can be set.

    Real llama-server / TabbyAPI use chunked transfer encoding; this
    test server uses Content-Length with the full body. The client's
    `for line in resp:` iterator works identically against either
    transport because urllib buffers either way before yielding lines.
    The behaviour we care about — parsing, indexing, end-of-text
    derivation — is independent of the on-wire framing.
    """
    parts: list[bytes] = []
    if recipe.raw_data_payloads is not None:
        for payload in recipe.raw_data_payloads:
            parts.append(f"data: {payload}\n\n".encode())
    else:
        for content, fr in recipe.chunks:
            parts.append(_sse_for_chunk(content, fr))
    if recipe.include_done:
        parts.append(b"data: [DONE]\n\n")
    return b"".join(parts)


class _Handler(BaseHTTPRequestHandler):
    """Per-server handler. The recipe is injected via a subclass attr."""

    # Set by `streaming_server()` via `type(...)`. Concrete subclasses
    # always have a real recipe; this annotation keeps mypy happy.
    recipe: StreamRecipe = StreamRecipe()

    def log_message(self, *_args: Any) -> None:  # type: ignore[override]
        # Silence the per-request log lines that BaseHTTPRequestHandler
        # otherwise dumps to stderr during the test run.
        return

    def do_GET(self) -> None:
        if self.path == "/health":
            self._respond_json(200, self.recipe.health_body or {"status": "ok"})
            return
        if self.path == "/props":
            self._respond_json(
                200,
                self.recipe.props_body
                or {
                    "model_path": "test.gguf",
                    "default_generation_settings": {"n_ctx": 8192},
                },
            )
            return
        if self.path == "/v1/models":
            self._respond_json(
                200,
                self.recipe.models_body or {"data": [{"id": "test-model"}]},
            )
            return
        self.send_error(404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            body = {}

        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return

        if not body.get("stream"):
            self._respond_json(
                200,
                self.recipe.non_stream_body
                or {
                    "choices": [{
                        "message": {"content": "fallback"},
                        "finish_reason": "stop",
                    }],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                },
            )
            return

        body_bytes = _build_stream_body(self.recipe)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        try:
            self.wfile.write(body_bytes)
            self.wfile.flush()
        except BrokenPipeError:
            # Client disconnected mid-write; tests that exercise that
            # path (none today) would close the response early.
            pass

    def _respond_json(self, code: int, payload: dict[str, Any]) -> None:
        data = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


@contextmanager
def streaming_server(recipe: StreamRecipe) -> Iterator[str]:
    """Run a real HTTP server on a free localhost port. Yields base URL.

    Cleans up on context exit so tests can run in parallel without
    sharing state. Each invocation creates its own `_Handler` subclass
    so the recipe is bound to that server instance, not a shared
    class-level slot.
    """
    handler_cls = type("BoundHandler", (_Handler,), {"recipe": recipe})
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
