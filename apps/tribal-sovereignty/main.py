"""Tribal Data Sovereignty Demo.

Shows the OCAP path: a TrustClaim with allowed_routes=["local_only"]
governs every operation. The app refuses (without going to the wire)
to send protected data anywhere except the local MAI instance.

Once the live cloud trust bridge is available, the manual TrustClaim construction here is replaced
by ``client.auth.exchange_token(claim_from_cloud_bridge)``.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path
from typing import Any

from mai import (
    ChatMessage,
    MaiClient,
    MaiClientConfig,
    MaiError,
    TrustClaim,
)

DEFAULT_CONFIG = Path(__file__).with_name("config.toml")


# ---------------------------------------------------------------------------
# Sovereignty guard
# ---------------------------------------------------------------------------

class SovereigntyViolation(RuntimeError):  # noqa: N818  see TRIBAL-SOV-NAMING below
    """Raised when an operation would leave the locally-allowed route set.

    Naming note (J-10b): ruff N818 prefers `SovereigntyViolationError`.
    The current name is referenced by the OCAP module wire surface and
    by every test in `apps/tribal-sovereignty/tests/`. Renaming touches
    the public API and is deliberately deferred to a dedicated rename
    session rather than bundled into a lint sweep.
    """


def claim_from_config(trust_cfg: dict[str, Any]) -> TrustClaim:
    """Build a TrustClaim from TOML. Mock."""
    return TrustClaim(
        claim_id=trust_cfg.get("claim_id", "local-dev-claim"),
        tenant_id=trust_cfg.get("tenant_id", "local-dev"),
        subject_id=trust_cfg.get("subject_id", "subject"),
        subject_hash=trust_cfg.get("subject_hash", "hmac-placeholder"),
        roles=list(trust_cfg.get("roles", [])),
        compliance_scopes=list(trust_cfg.get("compliance_scopes", [])),
        allowed_routes=list(trust_cfg.get("allowed_routes", ["local_only"])),
        allowed_models=list(trust_cfg.get("allowed_models", [])),
        max_data_classification=trust_cfg.get("max_data_classification", "restricted"),
        service_identity=trust_cfg.get("service_identity", "lamprey-router"),
        trust_bundle_version=trust_cfg.get("trust_bundle_version", "local-dev"),
        offline_mode=bool(trust_cfg.get("offline_mode", False)),
        revocation_status=trust_cfg.get("revocation_status", "unknown"),
    )


def guard_route(claim: TrustClaim, intended_route: str) -> None:
    """Raise SovereigntyViolation if intended_route isn't in claim.allowed_routes.

    `intended_route` is one of: local_only, local_preferred, cloud_allowed.
    """
    if intended_route not in claim.allowed_routes:
        raise SovereigntyViolation(
            f"route '{intended_route}' not in allowed_routes "
            f"{claim.allowed_routes} for tenant {claim.tenant_id} "
            f"(scopes={claim.compliance_scopes})",
        )


def guard_model(claim: TrustClaim, model: str) -> None:
    """Raise if the model isn't in the claim's allowed_models (when set)."""
    if claim.allowed_models and model not in claim.allowed_models:
        raise SovereigntyViolation(
            f"model '{model}' not in allowed_models {claim.allowed_models} "
            f"for tenant {claim.tenant_id}",
        )


# ---------------------------------------------------------------------------
# Pipeline
# ---------------------------------------------------------------------------

def load_app_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    if not path.exists():
        return {}
    with path.open("rb") as fh:
        return tomllib.load(fh)


def load_corpus(data_dir: Path) -> list[tuple[str, str]]:
    """Return list of (filename, content). Tagged as tribal_protected."""
    if not data_dir.exists():
        return []
    out: list[tuple[str, str]] = []
    for path in sorted(data_dir.iterdir()):
        if path.suffix.lower() in {".txt", ".md"}:
            out.append((path.name, path.read_text(encoding="utf-8")))
    return out


def _make_client(sdk_config: MaiClientConfig) -> MaiClient:
    return MaiClient(sdk_config)


def run(prompt: str, *, config_path: Path = DEFAULT_CONFIG,
        intended_route: str = "local_only",
        dry_run: bool = False) -> int:
    cfg = load_app_config(config_path)
    trust_cfg = cfg.get("trust", {})
    corpus_cfg = cfg.get("corpus", {})
    chat_cfg = cfg.get("chat", {})
    client_overrides = cfg.get("client", {})

    claim = claim_from_config(trust_cfg)
    print(json.dumps({
        "claim": {
            "tenant_id": claim.tenant_id,
            "subject_id": claim.subject_id,
            "scopes": claim.compliance_scopes,
            "allowed_routes": claim.allowed_routes,
            "allowed_models": claim.allowed_models,
            "service_identity": claim.service_identity,
            "trust_bundle_version": claim.trust_bundle_version,
        },
    }, indent=2), file=sys.stderr)

    chat_model = chat_cfg.get("chat_model", "qwen3-14b:Q4_K_M")

    # Two guards in sequence; either failure exits cleanly.
    try:
        guard_route(claim, intended_route)
        guard_model(claim, chat_model)
    except SovereigntyViolation as e:
        print(f"sovereignty violation: {e}", file=sys.stderr)
        return 4

    data_dir = (config_path.parent / corpus_cfg.get("data_dir", "protected_data")).resolve()
    corpus = load_corpus(data_dir)

    system = (
        "You are an assistant working with OCAP-governed cultural materials.\n"
        f"Tenant: {claim.tenant_id}. Subject: {claim.subject_id}.\n"
        f"Scopes: {','.join(claim.compliance_scopes)}.\n"
        "Treat all corpus content as tribal_protected; do not summarize\n"
        "outside the local scope.\n\n"
        "Corpus excerpts:\n"
        + "\n\n".join(f"[{name}]\n{content[:400]}" for name, content in corpus)
    )
    messages = [
        ChatMessage(role="system", content=system),
        ChatMessage(role="user", content=prompt),
    ]

    if dry_run:
        # Show what would be sent, do not call the server.
        print(json.dumps({
            "route": intended_route,
            "model": chat_model,
            "corpus_items": [name for name, _ in corpus],
            "system_chars": len(system),
        }, indent=2))
        return 0

    sdk_config = MaiClientConfig.load(**client_overrides)
    with _make_client(sdk_config) as client:
        try:
            resp = client.chat(
                chat_model, messages,
                temperature=float(chat_cfg.get("temperature", 0.3)),
                max_tokens=int(chat_cfg.get("max_tokens", 256)),
            )
        except MaiError as e:
            print(f"chat failed ({type(e).__name__}): {e}", file=sys.stderr)
            return 3
    print(resp.choices[0].message.content)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="tribal-sovereignty",
        description="OCAP-aware local-only chat against tribal protected data.",
    )
    parser.add_argument("prompt")
    parser.add_argument("--config", default=str(DEFAULT_CONFIG))
    parser.add_argument("--intended-route", default="local_only",
                        choices=("local_only", "local_preferred", "cloud_allowed"),
                        help="route the app would dispatch to (guard enforces)")
    parser.add_argument("--dry-run", action="store_true",
                        help="show planned request, do not call MAI")
    args = parser.parse_args(argv)
    return run(
        args.prompt, config_path=Path(args.config),
        intended_route=args.intended_route, dry_run=args.dry_run,
    )


if __name__ == "__main__":
    sys.exit(main())
