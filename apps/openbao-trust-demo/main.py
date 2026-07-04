"""OpenBao-Backed Local Trust Demo.

Walks the seven-step Trust Manifold pipeline end-to-end against a local
MAI instance:

    1. ``simulate_bridge_authentication()`` — the cloud OpenBao trust
       bridge would normally mint a short-lived ``TrustClaim``. This
       function emits one with the wire shape the bridge will use.
    2. ``audit_correlation_id()`` — derives a stable per-session ID
       from the claim, suitable for joining the audit log.
    3. ``check_local_trust_bundle()`` — calls
       ``client.trust.bundle_status()``.
       Falls back to an ``"unreachable"`` snapshot if the server is down
       so the audit summary still emits a stable shape.
    4. ``exchange_for_session_token()`` — calls
       ``client.auth.exchange_token(claim.subject_id,...)`` (live
       endpoint). Falls back to a claim-derived placeholder token if the
       server is unreachable.
    5. ``build_lamprey_metadata()`` — assembles the audit payload that
       the ``AuditFeed`` consumes (claim_id, tenant_id, subject_hash,
       service_identity, trust_bundle_version, route_decision,
       correlation_id).
    6. ``run_inference()`` — sends one authenticated chat completion.
    7. ``print_audit_summary()`` — emits the metadata + correlation
       ID as JSON to stdout for replay tooling.

Steps 3 and 4 hit real local endpoints; step 1 still simulates the
cloud OpenBao trust bridge until that bring-up.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
import tomllib
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from mai import ChatMessage, MaiClient, MaiClientConfig, MaiError, TrustClaim

DEFAULT_CONFIG = Path(__file__).with_name("config.toml")


# ---------------------------------------------------------------------------
# Step 1: simulated trust-bridge authentication
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class BridgeResult:
    """What the (simulated) cloud trust bridge returns."""
    claim: TrustClaim
    issued_at: int
    expires_at: int


def _subject_hash(tenant_id: str, subject_id: str) -> str:
    """HMAC-style pseudonymization marker.

    Real HMAC subject hashing lives in Rust; the SDK side does not have
    it yet. Until then we use a stable SHA-256 prefix so the wire
    shape is correct and tests can assert on it.
    """
    digest = hashlib.sha256(f"{tenant_id}|{subject_id}".encode()).hexdigest()
    return f"sha256:{digest[:32]}"


def simulate_bridge_authentication(
    bridge_cfg: dict[str, Any], claim_cfg: dict[str, Any], *,
    now: int | None = None,
) -> BridgeResult:
    """Mint a short-lived TrustClaim. Stand-in for the cloud bridge."""
    issued = int(time.time()) if now is None else now
    ttl = int(bridge_cfg.get("claim_ttl_seconds", 300))
    expires = issued + ttl if ttl > 0 else 0

    tenant_id = str(claim_cfg.get("tenant_id", "im-demo"))
    subject_id = str(claim_cfg.get("subject_id", "subject@example"))

    claim = TrustClaim(
        claim_id=f"claim-{uuid.uuid4()}",
        tenant_id=tenant_id,
        subject_id=subject_id,
        subject_hash=_subject_hash(tenant_id, subject_id),
        roles=list(claim_cfg.get("roles", [])),
        compliance_scopes=list(claim_cfg.get("compliance_scopes", [])),
        allowed_routes=list(claim_cfg.get("allowed_routes", ["local_only"])),
        allowed_models=list(claim_cfg.get("allowed_models", [])),
        max_data_classification=str(claim_cfg.get("max_data_classification",
                                                  "restricted")),
        service_identity=str(bridge_cfg.get("service_identity",
                                            "openbao-trust-bridge")),
        trust_bundle_version=str(bridge_cfg.get("trust_bundle_version",
                                                "local-dev")),
        offline_mode=False,
        revocation_status="active",
        issued_at_unix=issued,
        expires_at_unix=expires,
    )
    return BridgeResult(claim=claim, issued_at=issued, expires_at=expires)


# ---------------------------------------------------------------------------
# Step 2: audit correlation
# ---------------------------------------------------------------------------

def audit_correlation_id(claim: TrustClaim, prefix: str) -> str:
    """Stable per-claim correlation ID. Format: ``<prefix>-<claim_id>``.

    Joins the cloud-side claim with the local audit log. The audit log's
    ``AuditEntry`` keys on this string in addition to ``claim_id``.
    """
    return f"{prefix}-{claim.claim_id}"


# ---------------------------------------------------------------------------
# Step 3: local trust cache state
# ---------------------------------------------------------------------------

@dataclass
class BundleSnapshot:
    """Either a real ``TrustBundleStatus`` summary or a stub fallback."""
    state: str  # "live" or "stub"
    bundle_version: str
    connectivity: str
    signature_verified: bool
    detail: str = ""


def check_local_trust_bundle(client: MaiClient,
                             fallback_version: str) -> BundleSnapshot:
    """Query the SDK's trust bundle status.

    The endpoint is wired live, so a healthy server always returns
    a real ``TrustBundleStatus``. The fallback branch is the air-gap /
    server-down posture: the demo must still emit an audit-ready summary
    even if the local trust cache is unreachable.
    """
    try:
        st = client.trust.bundle_status()
    except MaiError as e:
        return BundleSnapshot(
            state="unreachable", bundle_version=fallback_version,
            connectivity="error", signature_verified=False,
            detail=f"{type(e).__name__}: {e}",
        )
    return BundleSnapshot(
        state="live", bundle_version=st.bundle_version or fallback_version,
        connectivity=st.connectivity,
        signature_verified=not st.is_emergency_only,
    )


# ---------------------------------------------------------------------------
# Step 4: claim -> session token
# ---------------------------------------------------------------------------

def exchange_for_session_token(client: MaiClient, claim: TrustClaim) -> str:
    """Trade the claim for a server session token via the live
    ``POST /v1/auth/exchange_token`` endpoint.

    Falls back to a claim-derived placeholder if the server is unreachable
    so the audit summary still carries a stable correlation marker. The
    local-dev token handler returns ``mode = "local-dev"``; production
    OpenBao deployment replaces only the handler body, not the wire shape.
    """
    try:
        resp = client.auth.exchange_token(
            claim.subject_id,
            tenant_id=claim.tenant_id,
            scopes=list(claim.compliance_scopes),
        )
    except MaiError:
        return f"local-fallback:{claim.claim_id}"
    return resp.token


# ---------------------------------------------------------------------------
# Step 5: Lamprey audit metadata
# ---------------------------------------------------------------------------

@dataclass
class LampreyMetadata:
    claim_id: str
    tenant_id: str
    subject_hash: str
    service_identity: str
    trust_bundle_version: str
    route_decision: str
    correlation_id: str
    bundle_state: str
    bundle_connectivity: str
    bundle_signature_verified: bool
    extras: dict[str, Any] = field(default_factory=dict)


def build_lamprey_metadata(claim: TrustClaim, *,
                           bundle: BundleSnapshot,
                           correlation_id: str,
                           route_decision: str = "local_only") -> LampreyMetadata:
    """Assemble the audit payload the audit log ingests."""
    return LampreyMetadata(
        claim_id=claim.claim_id,
        tenant_id=claim.tenant_id,
        subject_hash=claim.subject_hash,
        service_identity=claim.service_identity,
        trust_bundle_version=claim.trust_bundle_version,
        route_decision=route_decision,
        correlation_id=correlation_id,
        bundle_state=bundle.state,
        bundle_connectivity=bundle.connectivity,
        bundle_signature_verified=bundle.signature_verified,
    )


# ---------------------------------------------------------------------------
# Step 6 + 7: inference + audit print
# ---------------------------------------------------------------------------

def run_inference(client: MaiClient, *, model: str, prompt: str,
                  metadata: LampreyMetadata,
                  temperature: float, max_tokens: int) -> str:
    """Send one chat completion. The metadata is included as a system
    block so the model can see who is asking (and a future audit middleware
    can lift it back out of the request)."""
    system = (
        "You are operating under a verified local trust claim.\n"
        f"tenant_id={metadata.tenant_id}\n"
        f"service_identity={metadata.service_identity}\n"
        f"route={metadata.route_decision}\n"
        f"correlation_id={metadata.correlation_id}\n"
    )
    messages = [
        ChatMessage(role="system", content=system),
        ChatMessage(role="user", content=prompt),
    ]
    resp = client.chat(model, messages, temperature=temperature,
                       max_tokens=max_tokens)
    return resp.choices[0].message.content


def print_audit_summary(metadata: LampreyMetadata) -> None:
    payload = {
        "claim_id": metadata.claim_id,
        "tenant_id": metadata.tenant_id,
        "subject_hash": metadata.subject_hash,
        "service_identity": metadata.service_identity,
        "trust_bundle_version": metadata.trust_bundle_version,
        "route_decision": metadata.route_decision,
        "correlation_id": metadata.correlation_id,
        "bundle_state": metadata.bundle_state,
        "bundle_connectivity": metadata.bundle_connectivity,
        "bundle_signature_verified": metadata.bundle_signature_verified,
    }
    print(json.dumps(payload, indent=2))


# ---------------------------------------------------------------------------
# Top-level driver
# ---------------------------------------------------------------------------

def load_app_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    if not path.exists():
        return {}
    with path.open("rb") as fh:
        return tomllib.load(fh)


def _make_client(sdk_config: MaiClientConfig) -> MaiClient:
    """Indirection hook so tests can inject a MockTransport-backed client."""
    return MaiClient(sdk_config)


def run(*, config_path: Path = DEFAULT_CONFIG,
        prompt: str | None = None, dry_run: bool = False) -> int:
    cfg = load_app_config(config_path)
    bridge_cfg = cfg.get("bridge", {})
    claim_cfg = cfg.get("claim", {})
    audit_cfg = cfg.get("audit", {})
    chat_cfg = cfg.get("chat", {})
    client_overrides = cfg.get("client", {})

    # Step 1
    bridge = simulate_bridge_authentication(bridge_cfg, claim_cfg)
    claim = bridge.claim
    print(f"[step 1] bridge issued claim {claim.claim_id} "
          f"(expires_at={bridge.expires_at})", file=sys.stderr)

    if bridge.expires_at and bridge.expires_at <= bridge.issued_at:
        print("bridge issued an already-expired claim; aborting",
              file=sys.stderr)
        return 5

    # Step 2
    correlation_id = audit_correlation_id(
        claim, str(audit_cfg.get("correlation_prefix", "openbao-demo")),
    )
    print(f"[step 2] correlation_id={correlation_id}", file=sys.stderr)

    sdk_config = MaiClientConfig.load(**client_overrides)
    with _make_client(sdk_config) as client:
        # Step 3
        bundle = check_local_trust_bundle(
            client, str(bridge_cfg.get("trust_bundle_version", "local-dev")),
        )
        print(f"[step 3] local trust bundle: state={bundle.state} "
              f"connectivity={bundle.connectivity}", file=sys.stderr)

        # Step 4
        session_token = exchange_for_session_token(client, claim)
        print(f"[step 4] session_token={session_token[:32]}...",
              file=sys.stderr)

        # Step 5
        metadata = build_lamprey_metadata(
            claim, bundle=bundle, correlation_id=correlation_id,
            route_decision=(claim.allowed_routes[0]
                            if claim.allowed_routes else "local_only"),
        )

        if dry_run:
            print("[dry-run] skipping inference", file=sys.stderr)
            print_audit_summary(metadata)
            return 0

        # Step 6
        try:
            reply = run_inference(
                client,
                model=str(chat_cfg.get("chat_model", "qwen3-14b:Q4_K_M")),
                prompt=prompt or str(chat_cfg.get("prompt", "Hello.")),
                metadata=metadata,
                temperature=float(chat_cfg.get("temperature", 0.3)),
                max_tokens=int(chat_cfg.get("max_tokens", 128)),
            )
        except MaiError as e:
            print(f"[step 6] inference failed ({type(e).__name__}): {e}",
                  file=sys.stderr)
            return 3
        print(reply)

    # Step 7
    print_audit_summary(metadata)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="openbao-trust-demo",
        description="OpenBao-backed local trust demo (Plan §777).",
    )
    parser.add_argument("--prompt", default=None,
                        help="user prompt (default: from config.toml)")
    parser.add_argument("--config", default=str(DEFAULT_CONFIG),
                        help="path to config.toml")
    parser.add_argument("--dry-run", action="store_true",
                        help="walk the pipeline without sending inference")
    args = parser.parse_args(argv)
    return run(config_path=Path(args.config),
               prompt=args.prompt, dry_run=args.dry_run)


if __name__ == "__main__":
    sys.exit(main())
