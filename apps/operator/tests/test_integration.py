"""Integration: full dashboard render against a fully-mocked server."""

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
        "operator_int_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _all_panels_handler(req: httpx.Request) -> httpx.Response:
    path = req.url.path
    if path == "/v1/health":
        return httpx.Response(200, json={
            "status": "healthy", "air_gap_verified": True,
            "power_state": "full_inference", "uptime_seconds": 1,
            "adapters": {}, "hardware": {}, "system": {},
        })
    if path == "/v1/models":
        return httpx.Response(200, json={"data": [
            {"id": "m1", "object": "model", "created": 0, "name": "m1",
             "version": "v", "format": "GGUF",
             "size_bytes": 1, "required_vram_bytes": 1, "status": "loaded",
             "capabilities": {"chat": True, "completion": False,
                              "embedding": False, "vision": False,
                              "structured_output": False,
                              "max_context_tokens": 4096,
                              "supported_languages": []}},
        ]})
    if path == "/v1/scheduler/metrics":
        return httpx.Response(200, json={
            "queue_depth": 0, "active_requests": 1,
            "scheduled_total": 1, "rejected_total": 0,
            "avg_wait_ms": 1.0, "p95_wait_ms": 2.0,
            "instances": ["a"],
        })
    if path == "/v1/adapters":
        return httpx.Response(200, json=[{"id": "ad-1", "status": "healthy"}])
    if path == "/v1/power/state":
        return httpx.Response(200, json={
            "state": "full_inference", "estimated_power_watts": 200.0,
            "auto_demotion": {"enabled": False},
            "promotion_available": True,
            "promotion_latency_target_ms": 100,
        })
    if path == "/v1/system/airgap":
        return httpx.Response(200, json={
            "air_gap_enabled": True, "air_gap_verified": True,
            "network_state": "air_gap_compliant",
            "last_check_unix": 1, "violations_24h": 0,
        })
    if path == "/v1/audit/log":
        return httpx.Response(200, json={
            "total_entries": 0, "offset": 0, "limit": 10, "entries": [],
        })
    if path == "/v1/trust/status":
        return httpx.Response(200, json={
            "mode": "connected",
            "bundle_version": "bundle-2026-05-22",
            "last_refresh_secs": 1_700_000_000,
            "age_secs": 10,
            "claim_count": 0,
            "airgap": {"verified": True},
            "offline_backlog": 0,
        })
    return httpx.Response(404, json={"error": {
        "code": "MAI-N", "message": "?", "type": "internal_error",
    }})


def test_full_dashboard_json(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    def mk(_cfg: MaiClientConfig) -> MaiClient:
        c = MaiClient(MaiClientConfig(
            base_url="http://test/v1",
            retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
        ))
        c._http = httpx.Client(
            base_url="http://test/v1", headers={},
            transport=httpx.MockTransport(_all_panels_handler),
        )
        return c

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", mk)

    rc = main.run(config_path=APP_ROOT / "config.toml", as_json=True)
    out = capsys.readouterr().out
    assert rc == 0
    data = json.loads(out)
    panel_names = [p["name"] for p in data["panels"]]
    for required in ("models", "scheduler", "adapters", "power",
                     "airgap", "audit", "trust"):
        assert required in panel_names

    # Trust panel reports the live consolidated mode.
    trust_panel = next(p for p in data["panels"] if p["name"] == "trust")
    assert trust_panel["ok"] is True
    assert "mode=connected" in trust_panel["summary"]
    assert trust_panel["detail"]["bundle_version"] == "bundle-2026-05-22"


def test_unreachable_server_exits_one(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    def mk(_cfg: MaiClientConfig) -> MaiClient:
        def boom(_: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("dns")
        c = MaiClient(MaiClientConfig(
            base_url="http://test/v1",
            retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
        ))
        c._http = httpx.Client(
            base_url="http://test/v1", headers={},
            transport=httpx.MockTransport(boom),
        )
        return c

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", mk)

    rc = main.run(config_path=APP_ROOT / "config.toml")
    err = capsys.readouterr().err
    assert rc == 1
    assert "unreachable" in err
