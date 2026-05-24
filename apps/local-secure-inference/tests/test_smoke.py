"""Smoke test: scaffold starts, hits health, lists models, sends one chat."""

from __future__ import annotations

import importlib.util
import json
import sys
from collections.abc import Callable
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    """Load the dash-named app's main.py without importing as a package."""
    spec = importlib.util.spec_from_file_location(
        "local_secure_inference_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _make_mock_client(handler: Callable[[httpx.Request], httpx.Response]) -> MaiClient:
    cfg = MaiClientConfig(
        base_url="http://test/v1",
        retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
    )
    client = MaiClient(cfg)
    client._http = httpx.Client(
        base_url=cfg.base_url,
        headers=cfg.headers(),
        timeout=cfg.timeout,
        transport=httpx.MockTransport(handler),
    )
    return client


def test_scaffold_starts_and_completes_one_chat(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    chunks_sent: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/health":
            return httpx.Response(200, json={
                "status": "healthy", "air_gap_verified": True,
                "power_state": "full_inference", "uptime_seconds": 10,
                "adapters": {}, "hardware": {}, "system": {},
            })
        if req.url.path == "/v1/models":
            return httpx.Response(200, json={"data": [{
                "id": "test-chat", "object": "model", "created": 0,
                "name": "test-chat", "version": "v1",
                "format": "GGUF", "size_bytes": 1, "required_vram_bytes": 1,
                "status": "loaded",
                "capabilities": {"chat": True, "completion": False,
                                 "embedding": False, "vision": False,
                                 "structured_output": False,
                                 "max_context_tokens": 4096,
                                 "supported_languages": ["en"]},
            }]})
        if req.url.path == "/v1/chat/completions":
            sse = (
                'data: {"id":"1","object":"chat.completion.chunk","created":1,'
                '"model":"test-chat",'
                '"choices":[{"index":0,"delta":{"content":"hello"}}]}\n\n'
                "data: [DONE]\n\n"
            )
            chunks_sent.append(sse)
            return httpx.Response(200, content=sse.encode(),
                                  headers={"Content-Type": "text/event-stream"})
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "not found", "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _make_mock_client(handler))

    rc = main.run("hi", config_path=APP_ROOT / "config.toml")
    out = capsys.readouterr().out
    assert rc == 0
    assert "hello" in out
    assert len(chunks_sent) == 1


def test_health_failure_exits_nonzero(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("dns")

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _make_mock_client(handler))

    rc = main.run("hi", config_path=APP_ROOT / "config.toml")
    capsys.readouterr()
    assert rc == 1


def test_config_loader_handles_missing_file() -> None:
    main = _load_main()
    data = main.load_app_config(APP_ROOT / "does-not-exist.toml")
    assert data == {}


def test_config_loader_reads_real_file() -> None:
    main = _load_main()
    data = main.load_app_config(APP_ROOT / "config.toml")
    assert "chat" in data
    assert data["chat"]["model"] in ("auto", "qwen3-14b:Q4_K_M")
    # ensure no JSON pollution
    json.dumps(data)  # raises if anything weird snuck in
