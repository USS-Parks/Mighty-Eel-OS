"""Integration: end-to-end decision + dispatch + chat for each scope class."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import httpx
import pytest
from mai import MaiClient, MaiClientConfig
from mai.retry import RetryPolicy

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "compliance_routed_main_int", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _mock_client_returning(text: str) -> MaiClient:
    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/chat/completions":
            return httpx.Response(200, json={
                "id": "x", "object": "chat.completion", "created": 1,
                "model": "qwen3-14b:Q4_K_M",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1,
                          "total_tokens": 2},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "?", "type": "internal_error",
        }})

    c = MaiClient(MaiClientConfig(
        base_url="http://test/v1",
        retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
    ))
    c._http = httpx.Client(
        base_url="http://test/v1", headers={},
        transport=httpx.MockTransport(handler),
    )
    return c


@pytest.mark.parametrize(("scopes", "classification", "expected_flag"), [
    (["ocap"], "tribal_protected", "OCAP_REQUIRED"),
    (["itar_ear"], "controlled", "ITAR_EXPORT_CONTROL"),
    (["hipaa"], "phi", "PHI_PROTECTED"),
    ([], "public", None),
])
def test_full_pipeline_per_classification(
    scopes: list[str], classification: str, expected_flag: str | None,
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    main = _load_main()
    monkeypatch.setattr(main, "_make_client",
                        lambda _cfg: _mock_client_returning("ok"))

    rc = main.run("hello", config_path=APP_ROOT / "config.toml",
                  scopes=scopes, classification=classification)
    out, err = capsys.readouterr()
    assert rc == 0, err
    assert "ok" in out
    if expected_flag is not None:
        assert expected_flag in err
