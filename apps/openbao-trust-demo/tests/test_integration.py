"""Integration: full seven-step pipeline + verified-bundle path + refusal."""

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
    spec = importlib.util.spec_from_file_location(
        "openbao_trust_demo_main_int", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _mk_client(handler: Callable[[httpx.Request], httpx.Response]) -> MaiClient:
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


def test_full_pipeline_runs_end_to_end(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    """All seven steps against live endpoints. Chat must receive the
    audit metadata pinned into its system message and the audit summary
    must reflect the live bundle state."""
    chat_bodies: list[dict] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/trust/bundle_status":
            return httpx.Response(200, json={
                "bundle_version": "bundle-2026-05-22",
                "last_refresh_secs": 1_700_000_000,
                "age_secs": 60,
                "connectivity": "connected",
                "is_emergency_only": False,
            })
        if req.url.path == "/v1/auth/exchange_token":
            return httpx.Response(200, json={
                "token": "live-token-abc", "token_type": "Bearer",
                "subject_id": "alice@example.com", "tenant_id": "im-demo",
                "scopes": [], "issued_at_secs": 1_700_000_000,
                "expires_at_secs": 1_700_000_300, "mode": "local-dev",
            })
        if req.url.path == "/v1/chat/completions":
            chat_bodies.append(json.loads(req.content.decode()))
            return httpx.Response(200, json={
                "id": "id-1", "object": "chat.completion", "created": 1,
                "model": req.headers.get("x-model", "qwen3-14b:Q4_K_M"),
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant",
                                "content": "Confirmed local trust path."},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 30, "completion_tokens": 5,
                          "total_tokens": 35},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "not found",
            "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client",
                        lambda _cfg: _mk_client(handler))

    rc = main.run(config_path=APP_ROOT / "config.toml")
    out = capsys.readouterr().out
    assert rc == 0
    # body of stdout: chat reply followed by the audit summary JSON
    assert "Confirmed local trust path." in out
    # Extract the JSON tail (lines starting at the first '{')
    json_start = out.index("{")
    audit = json.loads(out[json_start:])
    assert audit["route_decision"] == "local_only"
    assert audit["service_identity"] == "openbao-trust-bridge"
    assert audit["bundle_state"] == "live"  # live endpoint
    assert audit["bundle_connectivity"] == "connected"
    assert audit["correlation_id"].startswith("openbao-demo-claim-")

    # The chat call had to carry the audit metadata in the system message.
    assert len(chat_bodies) == 1
    sys_msg = next(m["content"] for m in chat_bodies[0]["messages"]
                   if m["role"] == "system")
    assert "tenant_id=im-demo" in sys_msg
    assert "route=local_only" in sys_msg
    assert audit["correlation_id"] in sys_msg


def test_degraded_bundle_marks_signature_unverified(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """An emergency-only bundle (signature can't be re-verified) maps to
    ``signature_verified=False`` so the audit summary reflects the
    degraded mode the operator must see."""
    main = _load_main()

    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/trust/bundle_status"
        return httpx.Response(200, json={
            "bundle_version": "bundle-stale",
            "last_refresh_secs": 1_000,
            "age_secs": 999_999,
            "connectivity": "stale_not_expired",
            "is_emergency_only": True,
        })

    with _mk_client(handler) as client:
        snap = main.check_local_trust_bundle(client, "fallback-version")
    assert snap.state == "live"
    assert snap.bundle_version == "bundle-stale"
    assert snap.connectivity == "stale_not_expired"
    assert snap.signature_verified is False


def test_expired_claim_refused_with_exit_code_5(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """If the bridge mints an already-expired claim, the pipeline must
    refuse rather than dispatch inference."""
    main = _load_main()

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(500, json={"error": {
            "code": "MAI-X", "message": "should not be called",
            "type": "internal_error",
        }})

    # Patch the bridge to emit an expired claim.
    real_simulate = main.simulate_bridge_authentication

    def expired_simulate(bridge_cfg, claim_cfg, *, now=None):  # type: ignore[no-untyped-def]
        bridge_cfg = {**bridge_cfg, "claim_ttl_seconds": 0}
        result = real_simulate(bridge_cfg, claim_cfg, now=now)
        # Manually craft an already-expired claim by setting expires_at
        # in the past. dataclass is frozen, so build a fresh BridgeResult.
        new_claim = result.claim.model_copy(update={
            "issued_at_unix": 100, "expires_at_unix": 50,
        })
        return main.BridgeResult(claim=new_claim,
                                 issued_at=100, expires_at=50)

    monkeypatch.setattr(main, "simulate_bridge_authentication",
                        expired_simulate)
    monkeypatch.setattr(main, "_make_client",
                        lambda _cfg: _mk_client(handler))

    rc = main.run(config_path=APP_ROOT / "config.toml")
    err = capsys.readouterr().err
    assert rc == 5
    assert "already-expired" in err


def test_custom_prompt_overrides_config_default(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    """``--prompt`` (CLI / run() arg) wins over the config default."""
    seen_user_prompts: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/v1/trust/bundle_status":
            return httpx.Response(200, json={
                "bundle_version": "v", "connectivity": "connected",
                "is_emergency_only": False,
            })
        if req.url.path == "/v1/auth/exchange_token":
            return httpx.Response(200, json={
                "token": "t", "token_type": "Bearer",
                "subject_id": "s", "tenant_id": "im-demo",
                "scopes": [], "issued_at_secs": 1, "expires_at_secs": 2,
                "mode": "local-dev",
            })
        if req.url.path == "/v1/chat/completions":
            body = json.loads(req.content.decode())
            user = next(m["content"] for m in body["messages"]
                        if m["role"] == "user")
            seen_user_prompts.append(user)
            return httpx.Response(200, json={
                "id": "id-1", "object": "chat.completion", "created": 1,
                "model": "m",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1,
                          "total_tokens": 2},
            })
        return httpx.Response(404, json={"error": {
            "code": "MAI-N", "message": "not found",
            "type": "internal_error",
        }})

    main = _load_main()
    monkeypatch.setattr(main, "_make_client",
                        lambda _cfg: _mk_client(handler))

    rc = main.run(config_path=APP_ROOT / "config.toml",
                  prompt="What is my route?")
    capsys.readouterr()
    assert rc == 0
    assert seen_user_prompts == ["What is my route?"]


def test_correlation_id_is_per_claim_unique(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Each pipeline run should produce a distinct correlation_id even
    though the config is identical — UUIDs scope the claim_id."""
    main = _load_main()

    bridge_cfg = {"service_identity": "openbao-trust-bridge",
                  "trust_bundle_version": "v1", "claim_ttl_seconds": 60}
    claim_cfg = {"tenant_id": "t", "subject_id": "s",
                 "allowed_routes": ["local_only"]}

    r1 = main.simulate_bridge_authentication(bridge_cfg, claim_cfg)
    r2 = main.simulate_bridge_authentication(bridge_cfg, claim_cfg)
    cid1 = main.audit_correlation_id(r1.claim, "p")
    cid2 = main.audit_correlation_id(r2.claim, "p")
    assert cid1 != cid2
    assert r1.claim.claim_id != r2.claim.claim_id
    # tenant_id is the same so subject_hash must match across runs.
    assert r1.claim.subject_hash == r2.claim.subject_hash
