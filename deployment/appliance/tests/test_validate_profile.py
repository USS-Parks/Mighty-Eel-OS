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


def test_appliance_demo_passes_demo_profile() -> None:
    assert _rules(APPLIANCE_COMPOSE, "demo") == set()


def test_appliance_demo_is_rejected_as_production() -> None:
    assert "dev-mode" in _rules(APPLIANCE_COMPOSE, "production")


def test_unknown_profile_raises() -> None:
    with pytest.raises(ValueError, match="unknown profile"):
        vp.validate({"services": {}}, "staging")
