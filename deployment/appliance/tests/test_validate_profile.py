"""Regression tests for the appliance trust-plane profile validator (PSPR-01, AF-12).

Each unsafe fixture must be rejected with a specific rule; the secure fixture and
the hardened appliance demo must pass their intended profile; and the appliance
demo must still be rejected as production (it runs dev mode by design).
"""

from pathlib import Path

import pytest
import validate_profile as vp

_APPLIANCE = Path(__file__).resolve().parent.parent
FIXTURES = _APPLIANCE / "fixtures"
APPLIANCE_COMPOSE = _APPLIANCE / "docker-compose.yml"


def _rules(path: Path, profile: str) -> set[str]:
    return {v.rule for v in vp.validate(vp.load_compose(path), profile)}


@pytest.mark.parametrize(
    ("fixture", "profile", "expected_rule"),
    [
        ("unsafe-prod-dev-mode.yml", "production", "dev-mode"),
        ("unsafe-prod-known-token.yml", "production", "weak-credential"),
        ("unsafe-prod-host-published.yml", "production", "host-published-trust"),
        ("unsafe-demo-nonloopback.yml", "demo", "trust-exposed-nonloopback"),
        ("unsafe-demo-baked-token.yml", "demo", "weak-token"),
        ("unsafe-demo-not-gated.yml", "demo", "demo-not-gated"),
    ],
)
def test_unsafe_fixture_is_rejected(fixture: str, profile: str, expected_rule: str) -> None:
    rules = _rules(FIXTURES / fixture, profile)
    assert expected_rule in rules, f"{fixture} ({profile}): expected {expected_rule}, got {rules}"


def test_secure_production_fixture_passes() -> None:
    assert _rules(FIXTURES / "secure-production.yml", "production") == set()


def _rules_inline(compose: dict, profile: str) -> set[str]:
    return {v.rule for v in vp.validate(compose, profile)}


def test_trust_core_detected_via_entrypoint() -> None:
    # A retagged image whose only tell is the entrypoint must not evade
    # dev-mode detection.
    compose = {
        "services": {
            "kv": {
                "image": "registry.internal/kv-store:1",
                "entrypoint": ["/usr/local/bin/bao", "server", "-dev"],
            }
        }
    }
    assert "dev-mode" in _rules_inline(compose, "production")


def test_trust_core_detected_via_server_env_marker() -> None:
    compose = {
        "services": {
            "kv": {
                "image": "registry.internal/kv-store:1",
                "environment": {"BAO_LOCAL_CONFIG": "{}"},
                "ports": ["8200:8200"],
            }
        }
    }
    assert "host-published-trust" in _rules_inline(compose, "production")


def test_production_rejects_any_host_published_trust_port() -> None:
    # The cluster port (8201) is as much an exposure as the API port.
    compose = {
        "services": {
            "openbao": {
                "image": "openbao/openbao:latest",
                "command": "server",
                "ports": ["8201:8201"],
            }
        }
    }
    assert "host-published-trust" in _rules_inline(compose, "production")


def test_demo_rejects_nonloopback_on_any_trust_port() -> None:
    compose = {
        "services": {
            "openbao": {
                "image": "openbao/openbao:latest",
                "profiles": ["demo"],
                "command": "server -dev -dev-root-token-id=${TOKEN:?required}",
                "ports": ["0.0.0.0:8201:8201"],
            }
        }
    }
    assert "trust-exposed-nonloopback" in _rules_inline(compose, "demo")


def test_appliance_demo_passes_demo_profile() -> None:
    assert _rules(APPLIANCE_COMPOSE, "demo") == set()


def test_appliance_demo_is_rejected_as_production() -> None:
    assert "dev-mode" in _rules(APPLIANCE_COMPOSE, "production")


# The real shipped compositions — the same assertions CI runs
# (ship-validation.yml, compose-trust-validation job).

def test_wsf_ha_passes_production_profile() -> None:
    assert _rules(_APPLIANCE.parent / "wsf-ha" / "docker-compose.yml", "production") == set()


def test_shadow_passes_demo_and_is_rejected_as_production() -> None:
    shadow = _APPLIANCE.parent / "shadow" / "docker-compose.yml"
    assert _rules(shadow, "demo") == set()
    assert "dev-mode" in _rules(shadow, "production")


def test_unknown_profile_raises() -> None:
    with pytest.raises(ValueError, match="unknown profile"):
        vp.validate({"services": {}}, "staging")
