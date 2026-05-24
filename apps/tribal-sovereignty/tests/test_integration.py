"""Integration: full chat round trip under sovereignty guard."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "tribal_sovereignty_int", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_local_only_request_round_trips_with_claim_in_audit(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    captured: list[dict] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/chat/completions":
            captured.append(json.loads(req.content.decode()))
            return httpx.Response(200, json={
                "id": "x", "object": "chat.completion", "created": 1,
                "model": "qwen3-14b:Q4_K_M",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "I have reviewed the local corpus.",
                    },
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 50, "completion_tokens": 10,
                          "total_tokens": 60},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    def mk(_cfg: MaiClientConfig) -> MaiClient:
        c = MaiClient(MaiClientConfig(
            base_url="http://test/v1",
            retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
        ))
        c._http = httpx.Client(
            base_url="http://test/v1", headers={},
            transport=httpx.MockTransport(handler),
        )
        return c

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", mk)

    rc = main.run("What is in the corpus?", config_path=APP_ROOT / "config.toml")
    out, err = capsys.readouterr()
    assert rc == 0, err
    assert "I have reviewed the local corpus." in out

    # The system prompt should carry tenant/scope identifiers from the claim.
    assert len(captured) == 1
    system = next(m["content"] for m in captured[0]["messages"]
                  if m["role"] == "system")
    assert "nation-of-example" in system
    assert "ocap" in system
    # Stderr emitted the claim metadata
    assert "nation-of-example" in err
    assert "trust_bundle_version" in err
