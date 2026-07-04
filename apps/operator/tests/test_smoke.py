"""Smoke tests for operator dashboard."""

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
        "operator_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _mk(handler: Callable[[httpx.Request], httpx.Response]) -> MaiClient:
    cfg = MaiClientConfig(
        base_url="http://test/v1",
        retry=RetryPolicy(max_retries=0, base_delay=0.0, jitter=0.0),
    )
    c = MaiClient(cfg)
    c._http = httpx.Client(
        base_url=cfg.base_url, headers=cfg.headers(),
        timeout=cfg.timeout, transport=httpx.MockTransport(handler),
    )
    return c


# --- Panel: models -------------------------------------------------

def test_panel_models_counts_loaded() -> None:
    main = _load_main()

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/models":
            return httpx.Response(200, json={"data": [
                {"id": "m1", "object": "model", "created": 0, "name": "m1",
                 "version": "v", "format": "GGUF",
                 "size_bytes": 1, "required_vram_bytes": 1, "status": "loaded",
                 "capabilities": {"chat": True, "completion": False,
                                  "embedding": False, "vision": False,
                                  "structured_output": False,
                                  "max_context_tokens": 4096,
                                  "supported_languages": []}},
                {"id": "m2", "object": "model", "created": 0, "name": "m2",
                 "version": "v", "format": "GGUF",
                 "size_bytes": 1, "required_vram_bytes": 1,
                 "status": "cold_storage",
                 "capabilities": {"chat": True, "completion": False,
                                  "embedding": False, "vision": False,
                                  "structured_output": False,
                                  "max_context_tokens": 4096,
                                  "supported_languages": []}},
            ]})
        return httpx.Response(500)

    with _mk(handler) as client:
        p = main.panel_models(client)
    assert p.ok
    assert "1/2" in p.summary


# --- Panel: scheduler ----------------------------------------------

def test_panel_scheduler_summarizes_metrics() -> None:
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "queue_depth": 4, "active_requests": 2,
            "scheduled_total": 100, "rejected_total": 0,
            "avg_wait_ms": 8.0, "p95_wait_ms": 18.5,
            "instances": ["a", "b"],
        })

    with _mk(handler) as client:
        p = main.panel_scheduler(client)
    assert p.ok
    assert "queue=4" in p.summary
    assert "p95=18.5ms" in p.summary


# --- Panel: power ---------------------------------------------------

def test_panel_power_reports_state() -> None:
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "state": "sentinel", "estimated_power_watts": 12.5,
            "auto_demotion": {"enabled": True, "idle_minutes_remaining": 5,
                              "next_state": "deep_vault_sleep"},
            "promotion_available": True,
            "promotion_latency_target_ms": 5000,
        })

    with _mk(handler) as client:
        p = main.panel_power(client)
    assert p.ok
    assert "sentinel" in p.summary


# --- Panel: airgap --------------------------------------------------

def test_panel_airgap_marks_unverified_as_not_ok() -> None:
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "air_gap_enabled": True, "air_gap_verified": False,
            "network_state": "non_compliant",
            "last_check_unix": 1, "violations_24h": 3,
        })

    with _mk(handler) as client:
        p = main.panel_airgap(client)
    assert p.ok is False  # unverified -> ok=False
    assert "non_compliant" in p.summary


# Panel: trust --------------------------------------

def test_panel_trust_renders_bf6_live_status() -> None:
    main = _load_main()

    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/trust/status"
        return httpx.Response(200, json={
            "mode": "connected",
            "bundle_version": "bundle-2026-05-22",
            "last_refresh_secs": 1_700_000_000,
            "age_secs": 30,
            "claim_count": 4,
            "airgap": {"verified": True, "non_compliant": []},
            "offline_backlog": 0,
        })

    with _mk(handler) as client:
        p = main.panel_trust(client)
    assert p.ok is True
    assert "mode=connected" in p.summary
    assert "bundle-2026-05-22" in p.summary
    assert p.detail["mode"] == "connected"
    assert p.detail["claim_count"] == 4
    assert p.detail["offline_backlog"] == 0


def test_panel_trust_handles_server_unreachable() -> None:
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(503, json={"error": {
            "code": "MAI-503", "message": "trust offline",
            "type": "service_unavailable",
        }})

    with _mk(handler) as client:
        p = main.panel_trust(client)
    assert p.ok is False
    assert p.summary == "ERROR"


# --- Panel: audit ---------------------------------------------------

def test_panel_audit_renders_entries() -> None:
    main = _load_main()

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/audit/log":
            return httpx.Response(200, json={
                "total_entries": 3, "offset": 0, "limit": 3,
                "entries": [{
                    "timestamp": "2026-05-22T12:00:00",
                    "request_id": "00000000-0000-0000-0000-000000000001",
                    "profile_id": "00000000-0000-0000-0000-000000000001",
                    "endpoint": "/v1/chat/completions", "method": "POST",
                    "model": "qwen3", "tokens_in": 10, "tokens_out": 5,
                    "latency_ms": 100, "status_code": 200,
                    "priority": "normal", "hash": "", "prev_hash": "",
                }],
            })
        return httpx.Response(404)

    with _mk(handler) as client:
        p = main.panel_audit(client, 3)
    assert p.ok
    assert "3 total" in p.summary


# --- Error path ----------------------------------------------------

def test_safe_helper_catches_mai_error() -> None:
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(500, json={"error": {
            "code": "MAI-S001", "message": "boom",
            "type": "internal_error",
        }})

    with _mk(handler) as client:
        p = main.panel_models(client)
    assert p.ok is False
    assert "ServerError" in (p.error or "")


# --- Render --------------------------------------------------------

def test_render_text_marks_failures() -> None:
    main = _load_main()
    snap = main.DashboardSnapshot(panels=[
        main.Panel(name="models", ok=True, summary="2/2"),
        main.Panel(name="trust", ok=False, summary="not-provisioned",
                   error="stub"),
    ])
    text = main.render_text(snap)
    assert "[OK ] models" in text
    assert "[ERR] trust" in text
    assert "stub" in text


def test_run_returns_5_on_core_panel_failure(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/health":
            return httpx.Response(200, json={
                "status": "healthy", "air_gap_verified": True,
                "power_state": "full_inference", "uptime_seconds": 1,
                "adapters": {}, "hardware": {}, "system": {},
            })
        # Everything else fails 500 -> core panels error.
        return httpx.Response(500, json={"error": {
            "code": "MAI-S", "message": "boom",
            "type": "internal_error",
        }})

    cfg = tmp_path / "cfg.toml"
    cfg.write_text(
        "[display]\nmodels=true\nscheduler=true\nadapters=false\n"
        "power=true\naudit=false\ntrust=false\nairgap=true\n",
    )

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", lambda _cfg: _mk(handler))

    rc = main.run(config_path=cfg)
    out = capsys.readouterr().out
    assert rc == 5
    assert "[ERR] models" in out
