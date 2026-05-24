"""Integration tests for ``adapters.triton.client.TritonClient``.

These tests spin up a real in-process ``http.server`` on a free port
and drive the production ``TritonClient`` (real urllib opener, real
sockets, real JSON parsing) against it. They are the deterministic
local backend fakes the shared harness lock requires: the adapter
boundary is exercised end-to-end without touching a live Triton.

Markers:
- pure unit (no env vars, no live backend)
- safe to run in CI on any platform
"""

from __future__ import annotations

import json
import threading
from collections.abc import Callable, Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

import pytest

from adapters.base import (
    AdapterTimeoutError,
    BackendCrashedError,
    BackendUnavailableError,
    ContextExceededError,
    ModelNotFoundError,
    OutOfMemoryError,
    RateLimitedError,
    ValidationError,
)
from adapters.triton.client import (
    InferResponse,
    TritonClient,
    _extract_error_detail,
    _map_http_error,
)

# ─── Tiny configurable HTTP server ─────────────────────────────────────────


class _Route:
    """A single route the fake Triton recognises.

    ``handler`` returns ``(status, body_bytes, content_type)``.
    """

    def __init__(
        self,
        method: str,
        path: str,
        handler: Callable[[bytes], tuple[int, bytes, str]],
    ) -> None:
        self.method = method
        self.path = path
        self.handler = handler


class _State:
    """Shared mutable state for the fake server (per-fixture instance)."""

    def __init__(self) -> None:
        self.routes: list[_Route] = []
        self.request_log: list[tuple[str, str, bytes]] = []
        self.request_lock: threading.Lock = threading.Lock()


def _make_handler(state: _State) -> type[BaseHTTPRequestHandler]:
    class _Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args: Any, **_kw: Any) -> None:
            # Silence test-server stderr noise.
            return

        def _serve(self, method: str) -> None:
            length = int(self.headers.get("Content-Length", "0") or 0)
            body = self.rfile.read(length) if length else b""
            with state.request_lock:
                state.request_log.append((method, self.path, body))
            for route in state.routes:
                if route.method == method and route.path == self.path:
                    status, payload, ctype = route.handler(body)
                    self.send_response(status)
                    self.send_header("Content-Type", ctype)
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    return
            self.send_response(404)
            payload = json.dumps({"error": "no route"}).encode("utf-8")
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self) -> None:  # http.server requires this exact name
            self._serve("GET")

        def do_POST(self) -> None:  # http.server requires this exact name
            self._serve("POST")

    return _Handler


@pytest.fixture
def fake_triton() -> Iterator[tuple[str, _State]]:
    """Yield a (base_url, state) pair pointing at an in-process server."""
    state = _State()
    handler_cls = _make_handler(state)
    server = HTTPServer(("127.0.0.1", 0), handler_cls)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{port}", state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2.0)


def _json_route(method: str, path: str, status: int, body: dict[str, Any]) -> _Route:
    payload = json.dumps(body).encode("utf-8")
    return _Route(method, path, lambda _b: (status, payload, "application/json"))


def _empty_route(method: str, path: str, status: int = 200) -> _Route:
    return _Route(method, path, lambda _b: (status, b"", "application/json"))


# ─── Pooling / lifecycle ──────────────────────────────────────────────────


class TestPooling:
    def test_opener_reused_across_requests(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, state = fake_triton
        state.routes.append(_empty_route("GET", "/v2/health/live"))
        state.routes.append(_empty_route("GET", "/v2/health/ready"))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            opener_before = client.opener
            assert client.server_live() is True
            assert client.server_ready() is True
            opener_after = client.opener
            assert opener_before is opener_after
        finally:
            client.close()

    def test_close_is_idempotent(self, fake_triton: tuple[str, _State]) -> None:
        url, _ = fake_triton
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        client.close()
        client.close()  # second call must not raise
        with pytest.raises(BackendUnavailableError):
            _ = client.opener


# ─── Health probes ────────────────────────────────────────────────────────


class TestHealthProbes:
    def test_server_live_true(self, fake_triton: tuple[str, _State]) -> None:
        url, state = fake_triton
        state.routes.append(_empty_route("GET", "/v2/health/live"))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            assert client.server_live() is True
        finally:
            client.close()

    def test_server_live_false_when_backend_returns_500(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, state = fake_triton
        state.routes.append(_json_route(
            "GET", "/v2/health/live", 500, {"error": "boom"},
        ))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            assert client.server_live() is False
        finally:
            client.close()

    def test_server_live_false_when_connection_refused(self) -> None:
        # Address with no listener -> connect refused.
        client = TritonClient(
            base_url="http://127.0.0.1:1", timeout_ms=500, stream_timeout_ms=500,
        )
        try:
            assert client.server_live() is False
        finally:
            client.close()


# ─── Model readiness + metadata ───────────────────────────────────────────


class TestModelReadiness:
    def test_model_ready_true(self, fake_triton: tuple[str, _State]) -> None:
        url, state = fake_triton
        state.routes.append(_empty_route("GET", "/v2/models/yolo/ready"))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            assert client.model_ready("/v2/models/yolo") is True
        finally:
            client.close()

    def test_model_ready_false_on_404(self, fake_triton: tuple[str, _State]) -> None:
        url, _ = fake_triton  # no /ready route -> 404
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            assert client.model_ready("/v2/models/missing") is False
        finally:
            client.close()

    def test_model_metadata_returns_body(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, state = fake_triton
        state.routes.append(_json_route(
            "GET", "/v2/models/yolo", 200,
            {"name": "yolo", "platform": "tensorrt_plan"},
        ))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            meta = client.model_metadata("/v2/models/yolo")
            assert meta["name"] == "yolo"
            assert meta["platform"] == "tensorrt_plan"
        finally:
            client.close()

    def test_model_metadata_empty_dict_on_failure(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, _ = fake_triton
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            # No route registered -> 404 -> ModelNotFoundError -> {}
            assert client.model_metadata("/v2/models/nope") == {}
        finally:
            client.close()


# ─── Inference ────────────────────────────────────────────────────────────


class TestInfer:
    def test_infer_happy_path(self, fake_triton: tuple[str, _State]) -> None:
        url, state = fake_triton
        state.routes.append(_json_route(
            "POST", "/v2/models/yolo/infer", 200,
            {
                "model_name": "yolo",
                "outputs": [{
                    "name": "text_output",
                    "shape": [1],
                    "datatype": "BYTES",
                    "data": ["hello"],
                }],
            },
        ))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            resp = client.infer(
                "/v2/models/yolo",
                [{"name": "text_input", "shape": [1],
                  "datatype": "BYTES", "data": ["hi"]}],
                outputs=[{"name": "text_output"}],
                model_hint="yolo",
            )
        finally:
            client.close()
        assert isinstance(resp, InferResponse)
        assert resp.status_code == 200
        assert resp.body["outputs"][0]["data"] == ["hello"]

    def test_infer_empty_inputs_raises_validation(self) -> None:
        client = TritonClient(
            base_url="http://127.0.0.1:1", timeout_ms=500, stream_timeout_ms=500,
        )
        try:
            with pytest.raises(ValidationError):
                client.infer("/v2/models/yolo", [])
        finally:
            client.close()

    def test_infer_404_maps_to_model_not_found(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, _ = fake_triton
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            with pytest.raises(ModelNotFoundError) as info:
                client.infer(
                    "/v2/models/missing",
                    [{"name": "x", "shape": [1],
                      "datatype": "BYTES", "data": ["x"]}],
                    model_hint="missing",
                )
        finally:
            client.close()
        assert info.value.data["model"] == "missing"

    def test_infer_429_maps_to_rate_limited(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, state = fake_triton
        state.routes.append(_json_route(
            "POST", "/v2/models/yolo/infer", 429, {"error": "slow down"},
        ))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            with pytest.raises(RateLimitedError):
                client.infer(
                    "/v2/models/yolo",
                    [{"name": "x", "shape": [1],
                      "datatype": "BYTES", "data": ["x"]}],
                    model_hint="yolo",
                )
        finally:
            client.close()

    def test_infer_malformed_json_maps_to_backend_crashed(
        self, fake_triton: tuple[str, _State],
    ) -> None:
        url, state = fake_triton
        # 200 OK but body is not JSON.
        state.routes.append(_Route(
            "POST", "/v2/models/yolo/infer",
            lambda _b: (200, b"<<<not json>>>", "application/json"),
        ))
        client = TritonClient(base_url=url, timeout_ms=2000, stream_timeout_ms=2000)
        try:
            with pytest.raises(BackendCrashedError):
                client.infer(
                    "/v2/models/yolo",
                    [{"name": "x", "shape": [1],
                      "datatype": "BYTES", "data": ["x"]}],
                    model_hint="yolo",
                )
        finally:
            client.close()

    def test_unavailable_when_backend_not_listening(self) -> None:
        # Port 1 with no listener: on Linux this is ECONNREFUSED
        # (BackendUnavailableError); on Windows the SYN may time out
        # (AdapterTimeoutError). The shared contract treats both as
        # "backend can't be reached" -- the user gets a typed adapter
        # error either way -- so the test accepts both outcomes.
        client = TritonClient(
            base_url="http://127.0.0.1:1", timeout_ms=500, stream_timeout_ms=500,
        )
        try:
            with pytest.raises((BackendUnavailableError, AdapterTimeoutError)):
                client.infer(
                    "/v2/models/yolo",
                    [{"name": "x", "shape": [1],
                      "datatype": "BYTES", "data": ["x"]}],
                    model_hint="yolo",
                )
        finally:
            client.close()


# ─── _map_http_error / _extract_error_detail unit coverage ────────────────


class TestErrorMapping:
    def test_404_is_model_not_found(self) -> None:
        with pytest.raises(ModelNotFoundError) as info:
            _map_http_error(404, "{}", "yolo", 1.0)
        assert info.value.data["model"] == "yolo"

    def test_408_is_timeout(self) -> None:
        with pytest.raises(AdapterTimeoutError):
            _map_http_error(408, "{}", "yolo", 1.5)

    def test_504_is_timeout(self) -> None:
        with pytest.raises(AdapterTimeoutError):
            _map_http_error(504, "{}", "yolo", 0.25)

    def test_429_is_rate_limited(self) -> None:
        with pytest.raises(RateLimitedError):
            _map_http_error(429, "{}", "yolo", 1.0)

    def test_413_is_context_exceeded(self) -> None:
        with pytest.raises(ContextExceededError):
            _map_http_error(413, "{}", "yolo", 1.0)

    def test_oom_phrase_maps_to_oom(self) -> None:
        with pytest.raises(OutOfMemoryError):
            _map_http_error(500, '{"error":"CUDA out of memory"}', "yolo", 1.0)

    def test_502_is_backend_crashed(self) -> None:
        with pytest.raises(BackendCrashedError):
            _map_http_error(502, "{}", "yolo", 1.0)

    def test_generic_5xx_is_backend_unavailable(self) -> None:
        with pytest.raises(BackendUnavailableError):
            _map_http_error(503, "{}", "yolo", 1.0)

    def test_generic_4xx_is_backend_unavailable(self) -> None:
        with pytest.raises(BackendUnavailableError):
            _map_http_error(418, '{"error":"I am a teapot"}', "yolo", 1.0)

    def test_extract_detail_prefers_known_keys(self) -> None:
        assert _extract_error_detail('{"error":"bad"}') == "bad"
        assert _extract_error_detail('{"message":"msg"}') == "msg"
        assert _extract_error_detail('{"detail":"dt"}') == "dt"

    def test_extract_detail_truncates_plain_text(self) -> None:
        big = "x" * 500
        assert len(_extract_error_detail(big)) == 200

    def test_extract_detail_empty_returns_empty(self) -> None:
        assert _extract_error_detail("") == ""
