"""Compliance-Routed — placeholder router that mimics Lamprey output.

Until the server exposes ``/v1/compliance/decide``,
this scaffold evaluates a *local* mock rule table to produce the same
shape the real Lamprey policy runtime will emit. The shape mirrors
``mai-compliance::AggregateDecision`` so adapter code written today
won't need to change once the HTTP endpoint lands.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

from mai import ChatMessage, MaiClient, MaiClientConfig, MaiError

DEFAULT_CONFIG = Path(__file__).with_name("config.toml")


# ---------------------------------------------------------------------------
# Decision shape — mirrors mai-compliance::AggregateDecision
# ---------------------------------------------------------------------------

@dataclass
class RouteDecision:
    """What the real Lamprey policy runtime returns. Wire-compatible."""

    route: str  # local_only | local_preferred | cloud_allowed | deny
    flags: list[str] = field(default_factory=list)
    reason: str = ""
    matched_rule_index: int | None = None
    decided_by: str = "local-mock"  # "local-mock" today; "lamprey" once wired
    policy_version: str = "mock-v0"

    def is_denied(self) -> bool:
        return self.route == "deny"

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class RequestMetadata:
    """What an application would send to the real /v1/compliance/decide."""

    prompt: str
    compliance_scopes: list[str] = field(default_factory=list)
    data_classification: str = "public"
    tenant_id: str = "local-dev"


# ---------------------------------------------------------------------------
# Mock router (replace with client.compliance.decide())
# ---------------------------------------------------------------------------

class MockComplianceRouter:
    """Local rule evaluator. First-match-wins per config order.

    Replace with ``client.compliance.decide(metadata)`` once the server
    side lands. The decision shape is identical.
    """

    def __init__(self, rules: list[dict[str, Any]]) -> None:
        self._rules = rules

    def decide(self, metadata: RequestMetadata) -> RouteDecision:
        for idx, rule in enumerate(self._rules):
            scopes_required = set(rule.get("match_scopes", []))
            class_required = rule.get("match_class")
            actor_scopes = set(metadata.compliance_scopes)

            scope_ok = (
                not scopes_required  # rule has no scope requirement
                or scopes_required.issubset(actor_scopes)
            )
            class_ok = (
                class_required is None
                or class_required == metadata.data_classification
            )
            if scope_ok and class_ok:
                return RouteDecision(
                    route=rule.get("route", "local_preferred"),
                    flags=list(rule.get("flags", [])),
                    reason=rule.get("reason", ""),
                    matched_rule_index=idx,
                )
        # No rule matched — conservative default: deny.
        return RouteDecision(
            route="deny", flags=["NO_MATCHING_RULE"],
            reason="no rule matched; conservative deny",
        )


# ---------------------------------------------------------------------------
# Hook + driver
# ---------------------------------------------------------------------------

def load_app_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    if not path.exists():
        return {}
    with path.open("rb") as fh:
        return tomllib.load(fh)


def _make_client(sdk_config: MaiClientConfig) -> MaiClient:
    return MaiClient(sdk_config)


def execute(client: MaiClient, decision: RouteDecision, *,
            prompt: str, chat_model: str,
            temperature: float, max_tokens: int) -> str:
    """Honor the decision. ``local_only`` and ``local_preferred`` both
    dispatch to the local server in this scaffold (there's no cloud
    fallback to demo). ``deny`` raises; ``cloud_allowed`` would route
    to a different transport in a real app."""
    if decision.is_denied():
        raise RuntimeError(f"compliance deny: {decision.reason}")
    resp = client.chat(
        chat_model,
        [ChatMessage(role="user", content=prompt)],
        temperature=temperature, max_tokens=max_tokens,
    )
    return resp.choices[0].message.content


def run(prompt: str, *, config_path: Path = DEFAULT_CONFIG,
        scopes: list[str] | None = None,
        classification: str = "public",
        decision_only: bool = False) -> int:
    cfg = load_app_config(config_path)
    routing_cfg = cfg.get("routing", {})
    gen_cfg = cfg.get("generation", {})
    client_overrides = cfg.get("client", {})

    metadata = RequestMetadata(
        prompt=prompt,
        compliance_scopes=scopes or [],
        data_classification=classification,
    )

    router = MockComplianceRouter(routing_cfg.get("rules", []))
    decision = router.decide(metadata)

    print(json.dumps({"decision": decision.to_dict()}, indent=2), file=sys.stderr)

    if decision_only:
        return 0 if not decision.is_denied() else 4

    if decision.is_denied():
        print(f"refusing to dispatch: {decision.reason}", file=sys.stderr)
        return 4

    sdk_config = MaiClientConfig.load(**client_overrides)
    with _make_client(sdk_config) as client:
        try:
            text = execute(
                client, decision, prompt=prompt,
                chat_model=gen_cfg.get("chat_model", "qwen3-14b:Q4_K_M"),
                temperature=float(gen_cfg.get("temperature", 0.3)),
                max_tokens=int(gen_cfg.get("max_tokens", 256)),
            )
        except MaiError as e:
            print(f"chat failed ({type(e).__name__}): {e}", file=sys.stderr)
            return 3
        except RuntimeError as e:
            print(str(e), file=sys.stderr)
            return 4
    print(text)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="compliance-routed",
        description="Mock-Lamprey-routed chat using local rules only.",
    )
    parser.add_argument("prompt")
    parser.add_argument("--config", default=str(DEFAULT_CONFIG))
    parser.add_argument("--scope", action="append", dest="scopes", default=None,
                        help="compliance scope (repeatable, e.g. --scope hipaa)")
    parser.add_argument("--classification", default="public",
                        help="public | phi | controlled | tribal_protected")
    parser.add_argument("--decision-only", action="store_true",
                        help="emit the routing decision and exit; do not call MAI")
    args = parser.parse_args(argv)
    return run(
        args.prompt,
        config_path=Path(args.config),
        scopes=args.scopes,
        classification=args.classification,
        decision_only=args.decision_only,
    )


if __name__ == "__main__":
    sys.exit(main())
