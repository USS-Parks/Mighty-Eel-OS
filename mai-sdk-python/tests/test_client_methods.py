"""Sync client method coverage via httpx.MockTransport.

Tests every public method, retry behavior, error mapping, streaming
decode, and namespace dispatch. No real network is involved.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from typing import Any

import httpx
import pytest
from mai._namespaces import TrustNotProvisionedError
from mai.client import MaiClient
from mai.config import MaiClientConfig
from mai.errors import (
    AuthenticationError,
    NotFoundError,
    RateLimitError,
    ServerError,
)
from mai.retry import RetryPolicy
from mai.types import ChatMessage, PowerState, PowerTransitionRequest


def _client(handler: Callable[[httpx.Request], httpx.Response],
            *, retry: RetryPolicy | None = None) -> MaiClient:
    cfg = MaiClientConfig(
        base_url="http://test/v1",
        retry=retry or RetryPolicy(max_retries=2, base_delay=0.0,
                                    max_delay=0.01, jitter=0.0),
    )
    client = MaiClient(cfg)
    client._http = httpx.Client(  # swap transport
        base_url=cfg.base_url,
        headers=cfg.headers(),
        timeout=cfg.timeout,
        transport=httpx.MockTransport(handler),
    )
    return client


# ---------------------------------------------------------------------------
# Inference
# ---------------------------------------------------------------------------

def test_chat_returns_completion_response() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/chat/completions"
        body = json.loads(req.content.decode())
        assert body["model"] == "qwen3-14b:Q4_K_M"
        assert body["stream"] is False
        return httpx.Response(200, json={
            "id": "abc", "object": "chat.completion", "created": 1,
            "model": body["model"],
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        })

    with _client(handler) as client:
        r = client.chat("qwen3-14b:Q4_K_M",
                        [ChatMessage(role="user", content="hello")])
        assert r.choices[0].message.content == "hi"
        assert r.usage.total_tokens == 2


def test_chat_stream_yields_chunks_and_stops_on_done() -> None:
    chunks = [
        {"id": "1", "object": "chat.completion.chunk", "created": 1,
         "model": "m", "choices": [{"index": 0, "delta": {"content": "Hel"}}]},
        {"id": "1", "object": "chat.completion.chunk", "created": 1,
         "model": "m", "choices": [{"index": 0, "delta": {"content": "lo"}}]},
    ]
    sse = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"

    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=sse.encode(),
                              headers={"Content-Type": "text/event-stream"})

    with _client(handler) as client:
        collected = list(client.chat_stream(
            "m", [ChatMessage(role="user", content="hi")],
        ))
        assert len(collected) == 2
        assert collected[0].choices[0]["delta"]["content"] == "Hel"
        assert collected[1].choices[0]["delta"]["content"] == "lo"


def test_complete_alias_is_completions() -> None:
    assert MaiClient.complete is MaiClient.completions


def test_embed_alias_is_embeddings() -> None:
    assert MaiClient.embed is MaiClient.embeddings


def test_embed_returns_typed_response() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "object": "list",
            "model": "embed-1",
            "data": [{"object": "embedding", "index": 0,
                      "embedding": [0.1, 0.2], "input_tokens": 3}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 0, "total_tokens": 3},
        })

    with _client(handler) as client:
        r = client.embed("embed-1", ["hi"])
        assert len(r.data) == 1
        assert r.data[0].embedding == [0.1, 0.2]


# ---------------------------------------------------------------------------
# Models namespace
# ---------------------------------------------------------------------------

def test_models_list_filters_passed_as_query() -> None:
    captured: dict[str, Any] = {}

    def handler(req: httpx.Request) -> httpx.Response:
        captured["url"] = str(req.url)
        return httpx.Response(200, json={"data": []})

    with _client(handler) as client:
        client.models.list(family="qwen3")
        assert "family=qwen3" in captured["url"]


def test_models_load_returns_typed_response() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/models/foo/load"
        assert req.method == "POST"
        return httpx.Response(200, json={
            "model_id": "foo", "status": "loaded",
            "adapter_id": "ad-1", "gpu_id": "gpu-0",
            "vram_allocated_bytes": 1234, "load_time_ms": 50,
        })

    with _client(handler) as client:
        r = client.models.load("foo")
        assert r.model_id == "foo"
        assert r.load_time_ms == 50


def test_models_benchmark_includes_kwargs_body() -> None:
    captured: dict[str, Any] = {}

    def handler(req: httpx.Request) -> httpx.Response:
        captured["body"] = json.loads(req.content.decode())
        return httpx.Response(200, json={
            "model_id": "foo", "completed": True, "tokens_per_second": 42.0,
        })

    with _client(handler) as client:
        client.models.benchmark("foo", warmup=5)
        assert captured["body"] == {"warmup": 5}


# ---------------------------------------------------------------------------
# System / power / scheduler / updates
# ---------------------------------------------------------------------------

def test_power_get_state() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/power/state"
        return httpx.Response(200, json={
            "state": "full_inference",
            "estimated_power_watts": 220.0,
            "auto_demotion": {"enabled": False},
            "promotion_available": True,
            "promotion_latency_target_ms": 5000,
        })

    with _client(handler) as client:
        p = client.power.get_state()
        assert p.state == PowerState.FULL_INFERENCE


def test_power_transition_sends_request_body() -> None:
    captured: dict[str, Any] = {}

    def handler(req: httpx.Request) -> httpx.Response:
        captured["body"] = json.loads(req.content.decode())
        return httpx.Response(200, json={
            "from_state": "sentinel",
            "to_state": "full_inference",
            "accepted": True,
            "estimated_latency_ms": 100,
        })

    with _client(handler) as client:
        r = client.power.transition(PowerTransitionRequest(
            target_state=PowerState.FULL_INFERENCE,
            reason="user request",
        ))
        assert r.accepted
        assert captured["body"]["target_state"] == "full_inference"


def test_scheduler_metrics_round_trip() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "queue_depth": 3, "active_requests": 2,
            "scheduled_total": 100, "rejected_total": 1,
            "avg_wait_ms": 12.0, "p95_wait_ms": 50.0,
            "instances": ["i1", "i2"],
        })

    with _client(handler) as client:
        m = client.scheduler.metrics()
        assert m.queue_depth == 3
        assert m.instances == ["i1", "i2"]


def test_system_airgap_status() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/system/airgap"
        return httpx.Response(200, json={
            "air_gap_enabled": True, "air_gap_verified": True,
            "network_state": "air_gap_compliant",
            "last_check_unix": 1700000000,
        })

    with _client(handler) as client:
        s = client.system.airgap()
        assert s.air_gap_verified


def test_updates_check_returns_list() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "updates_available": True,
            "updates": [{
                "component": "mai-server", "current_version": "1.0",
                "target_version": "1.1", "size_bytes": 1024, "signed": True,
            }],
            "checked_at_unix": 1700000000,
        })

    with _client(handler) as client:
        r = client.updates.check()
        assert r.updates_available
        assert r.updates[0].component == "mai-server"


# ---------------------------------------------------------------------------
# Error mapping
# ---------------------------------------------------------------------------

def test_401_maps_to_authentication_error() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(401, json={"error": {
            "code": "MAI-A001", "message": "bad key",
            "type": "authentication_failed",
        }})

    with _client(handler) as client:
        with pytest.raises(AuthenticationError) as ei:
            client.health()
        assert ei.value.status_code == 401


def test_404_maps_to_not_found() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(404, json={"error": {
            "code": "MAI-N001", "message": "no model",
            "type": "internal_error",
        }})

    with _client(handler) as client:
        with pytest.raises(NotFoundError):
            client.models.get("nope")


def test_500_maps_to_server_error_and_retries() -> None:
    calls = {"n": 0}

    def handler(_: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        return httpx.Response(500, json={"error": {
            "code": "MAI-S001", "message": "boom",
            "type": "internal_error",
        }})

    # 5xx is retryable per the standard policy; exhausts max_retries=2 -> 3 calls
    with _client(handler, retry=RetryPolicy(max_retries=2, base_delay=0.0,
                                             max_delay=0.0, jitter=0.0)) as client:
        with pytest.raises(ServerError):
            client.models.list()
    assert calls["n"] == 3


# ---------------------------------------------------------------------------
# Retry behavior
# ---------------------------------------------------------------------------

def test_429_retries_then_succeeds() -> None:
    counts = {"n": 0}

    def handler(_: httpx.Request) -> httpx.Response:
        counts["n"] += 1
        if counts["n"] < 3:
            return httpx.Response(429, json={"error": {
                "code": "MAI-R001", "message": "slow down",
                "type": "rate_limited", "retry_after_seconds": 0,
            }})
        return httpx.Response(200, json={"data": []})

    with _client(handler) as client:
        models = client.models.list()
        assert models == []
        assert counts["n"] == 3


def test_429_exhausts_retries_and_raises() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(429, json={"error": {
            "code": "MAI-R001", "message": "limited",
            "type": "rate_limited", "retry_after_seconds": 0,
        }})

    with _client(handler) as client:
        with pytest.raises(RateLimitError):
            client.models.list()


def test_401_not_retried() -> None:
    counts = {"n": 0}

    def handler(_: httpx.Request) -> httpx.Response:
        counts["n"] += 1
        return httpx.Response(401, json={"error": {
            "code": "MAI-A001", "message": "bad",
            "type": "authentication_failed",
        }})

    with _client(handler) as client:
        with pytest.raises(AuthenticationError):
            client.models.list()
    assert counts["n"] == 1


# ---------------------------------------------------------------------------
# Health and reachability
# ---------------------------------------------------------------------------

def test_health_check_returns_false_on_failure() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("dns")

    with _client(handler) as client:
        assert client.health_check() is False


# ---------------------------------------------------------------------------
# Trust
# ---------------------------------------------------------------------------

def test_trust_status_decodes_envelope() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/trust/status"
        return httpx.Response(200, json={
            "mode": "air-gapped",
            "bundle_version": None,
            "last_refresh_secs": None,
            "age_secs": None,
            "claim_count": 0,
            "airgap": {"connectivity": "air-gapped", "permits_cloud_route": False,
                       "requires_local_only": True, "is_air_gapped": True},
            "offline_backlog": 0,
        })

    with _client(handler) as client:
        status = client.trust.status()
        assert status.mode == "air-gapped"
        assert status.claim_count == 0
        assert status.airgap["is_air_gapped"] is True


def test_trust_claims_returns_envelope() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/trust/claims"
        return httpx.Response(200, json={
            "claims": [
                {"claim_id": "c1", "status": "valid", "recorded_at_secs": 100},
                {"claim_id": "c2", "status": "revoked", "recorded_at_secs": 200},
            ],
            "total": 2,
        })

    with _client(handler) as client:
        env = client.trust.claims()
        assert env.total == 2
        assert env.claims[0].claim_id == "c1"
        assert env.claims[1].status == "revoked"


def test_trust_bundle_status_decodes_envelope() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={
            "bundle_version": "v1",
            "last_refresh_secs": 12345,
            "age_secs": 60,
            "connectivity": "connected",
            "is_emergency_only": False,
        })

    with _client(handler) as client:
        bs = client.trust.bundle_status()
        assert bs.bundle_version == "v1"
        assert bs.connectivity == "connected"
        assert bs.is_emergency_only is False


def test_trust_revocation_status_sends_query_param() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/trust/revocation_status"
        assert req.url.params.get("claim_id") == "c-42"
        return httpx.Response(200, json={"claim_id": "c-42", "status": "unknown"})

    with _client(handler) as client:
        r = client.trust.revocation_status("c-42")
        assert r.claim_id == "c-42"
        assert r.status == "unknown"


def test_auth_exchange_token_round_trips_body() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/auth/exchange_token"
        body = json.loads(req.content.decode())
        assert body["subject_id"] == "u-1"
        assert body["tenant_id"] == "t-a"
        assert body["scopes"] == ["local_only"]
        return httpx.Response(200, json={
            "token": "local-dev.admin.u-1.100",
            "token_type": "Bearer",
            "subject_id": "u-1",
            "tenant_id": "t-a",
            "scopes": ["local_only"],
            "issued_at_secs": 100,
            "expires_at_secs": 1000,
            "mode": "local-dev",
        })

    with _client(handler) as client:
        tok = client.auth.exchange_token("u-1", tenant_id="t-a", scopes=["local_only"])
        assert tok.token.startswith("local-dev.")
        assert tok.expires_at_secs == 1000


# ---------------------------------------------------------------------------
# Compliance
# ---------------------------------------------------------------------------

def test_compliance_get_status_decodes_envelope() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/compliance/status"
        return httpx.Response(200, json={
            "modules": [{"module": "hipaa", "enabled": True, "priority": 0}],
            "priority": ["hipaa"],
            "reload_count": 0,
            "audit_integrity": {
                "entry_count": 0, "chain_count": 0,
                "head_hash": "00" * 32, "last_verify": "unknown",
                "last_verify_error": None,
            },
            "subscribers": 0,
        })

    with _client(handler) as client:
        s = client.compliance.get_status()
        assert s.modules[0].module == "hipaa"
        assert s.audit_integrity.last_verify == "unknown"


def test_compliance_update_policy_sends_enabled() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.method == "PUT"
        assert req.url.path == "/v1/compliance/policies/hipaa"
        body = json.loads(req.content.decode())
        assert body == {"enabled": False}
        return httpx.Response(200, json={
            "module": "hipaa", "enabled": False, "changed": True,
        })

    with _client(handler) as client:
        r = client.compliance.update_policy("hipaa", enabled=False)
        assert r["changed"] is True


def test_compliance_audit_query_includes_filters() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        params = req.url.params
        assert params.get("module") == "hipaa"
        assert params.get("decision") == "deny"
        assert params.get("limit") == "100"
        return httpx.Response(200, json={"rows": [], "total": 0})

    with _client(handler) as client:
        env = client.compliance.query_audit(
            module="hipaa", decision="deny", limit=100,
        )
        assert env.total == 0


def test_compliance_generate_report_round_trip() -> None:
    record = {
        "id": "rep-1", "report_type": "system_activity",
        "status": "complete", "output_format": "json",
        "from_unix_nanos": 0, "to_unix_nanos": 1000,
        "tenant": "acme", "created_at_unix_nanos": 5,
        "completed_at_unix_nanos": 6, "content_hash_hex": "ab" * 32,
        "signature_hex": None, "error": None,
        "protected": False, "schedule_id": None,
    }

    def handler(req: httpx.Request) -> httpx.Response:
        assert req.method == "POST"
        assert req.url.path == "/v1/compliance/reports/generate"
        body = json.loads(req.content.decode())
        assert body["report_type"] == "system_activity"
        assert body["tenant"] == "acme"
        assert body["format"] == "json"
        return httpx.Response(200, json=record)

    with _client(handler) as client:
        r = client.compliance.generate_report(
            report_type="system_activity",
            from_unix_nanos=0, to_unix_nanos=1000, tenant="acme",
        )
        assert r.id == "rep-1"
        assert r.status == "complete"


def test_compliance_download_report_returns_bytes() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        assert req.url.path == "/v1/compliance/reports/rep-1/download"
        return httpx.Response(
            200, content=b"{\"hello\": true}",
            headers={"content-type": "application/json"},
        )

    with _client(handler) as client:
        body = client.compliance.download_report("rep-1")
        assert body == b'{"hello": true}'


def test_compliance_verify_audit_decodes_envelope() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"verified": True, "error": None})

    with _client(handler) as client:
        r = client.compliance.verify_audit()
        assert r["verified"] is True


# `TrustNotProvisionedError` is still exported for application code that
# wants to detect missing backends; the SDK no longer raises it from any
# trust namespace method.
def test_trust_not_provisioned_error_is_exported() -> None:
    assert issubclass(TrustNotProvisionedError, Exception)


# ---------------------------------------------------------------------------
# Factories
# ---------------------------------------------------------------------------

def test_from_env_constructs(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MAI_BASE_URL", "http://env:1234/v1")
    c = MaiClient.from_env()
    assert c._config.base_url == "http://env:1234/v1"
    c.close()


def test_namespaces_attached() -> None:
    with _client(lambda _r: httpx.Response(200)) as client:
        for ns in ("models", "power", "system", "scheduler",
                   "updates", "admin", "auth", "trust", "compliance"):
            assert hasattr(client, ns), f"missing namespace: {ns}"


def test_legacy_top_level_methods_still_work() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"data": []})

    with _client(handler) as client:
        assert client.list_models() == []


def test_streaming_error_raises_built_error() -> None:
    def handler(_: httpx.Request) -> httpx.Response:
        return httpx.Response(401, json={"error": {
            "code": "MAI-A001", "message": "bad key",
            "type": "authentication_failed",
        }})

    with _client(handler) as client:
        with pytest.raises(AuthenticationError):
            list(client.chat_stream("m", [ChatMessage(role="user", content="hi")]))
