"""Integration: pick_model resolution + multi-chunk streaming + metrics."""

from __future__ import annotations

import importlib.util
import sys
from collections.abc import Callable
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "local_secure_inference_main_int", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _mock_client(handler: Callable[[httpx.Request], httpx.Response]) -> MaiClient:
    cfg = MaiClientConfig(
        base_url="http://test/v1",
        retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
    )
    client = MaiClient(cfg)
    client._http = httpx.Client(
        base_url=cfg.base_url, headers=cfg.headers(),
        timeout=cfg.timeout, transport=httpx.MockTransport(handler),
    )
    return client


def test_auto_picks_first_chat_capable_model(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    selected: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/health":
            return httpx.Response(200, json={
                "status": "healthy", "air_gap_verified": True,
                "power_state": "full_inference", "uptime_seconds": 1,
                "adapters": {}, "hardware": {}, "system": {},
            })
        if req.url.path == "/v1/models":
            return httpx.Response(200, json={"data": [
                {  # non-chat embedding model, should be skipped
                    "id": "embed-only", "object": "model", "created": 0,
                    "name": "embed-only", "version": "v1", "format": "GGUF",
                    "size_bytes": 1, "required_vram_bytes": 1, "status": "loaded",
                    "capabilities": {"chat": False, "completion": False,
                                     "embedding": True, "vision": False,
                                     "structured_output": False,
                                     "max_context_tokens": 0,
                                     "supported_languages": []},
                },
                {
                    "id": "chat-7b", "object": "model", "created": 0,
                    "name": "chat-7b", "version": "v1", "format": "GGUF",
                    "size_bytes": 1, "required_vram_bytes": 1, "status": "loaded",
                    "capabilities": {"chat": True, "completion": False,
                                     "embedding": False, "vision": False,
                                     "structured_output": False,
                                     "max_context_tokens": 4096,
                                     "supported_languages": ["en"]},
                },
            ]})
        if req.url.path == "/v1/chat/completions":
            import json as _json
            body = _json.loads(req.content.decode())
            selected.append(body["model"])
            sse = (
                'data: {"id":"1","object":"chat.completion.chunk","created":1,'
                '"model":"' + body["model"] + '",'
                '"choices":[{"index":0,"delta":{"content":"part-1 "}}]}\n\n'
                'data: {"id":"1","object":"chat.completion.chunk","created":1,'
                '"model":"' + body["model"] + '",'
                '"choices":[{"index":0,"delta":{"content":"part-2"}}]}\n\n'
                "data: [DONE]\n\n"
            )
            return httpx.Response(200, content=sse.encode(),
                                  headers={"Content-Type": "text/event-stream"})
        if req.url.path == "/v1/scheduler/metrics":
            return httpx.Response(200, json={
                "queue_depth": 0, "active_requests": 1,
                "scheduled_total": 5, "rejected_total": 0,
                "avg_wait_ms": 1.0, "p95_wait_ms": 2.0,
                "instances": [],
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _mock_client(handler))

    rc = main.run("Hello", config_path=APP_ROOT / "config.toml")
    captured = capsys.readouterr()
    assert rc == 0
    assert selected == ["chat-7b"]
    assert "part-1 part-2" in captured.out
    # show_metrics = true in config -> stderr footer
    assert "scheduler:" in captured.err


def test_unknown_model_in_config_fails_cleanly(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/health":
            return httpx.Response(200, json={
                "status": "healthy", "air_gap_verified": True,
                "power_state": "full_inference", "uptime_seconds": 1,
                "adapters": {}, "hardware": {}, "system": {},
            })
        return httpx.Response(200, json={"data": []})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _mock_client(handler))

    cfg_file = tmp_path / "custom.toml"
    cfg_file.write_text('[chat]\nmodel = "nonexistent-99b"\n')

    rc = main.run("hi", config_path=cfg_file)
    err = capsys.readouterr().err
    assert rc == 2
    assert "nonexistent-99b" in err
