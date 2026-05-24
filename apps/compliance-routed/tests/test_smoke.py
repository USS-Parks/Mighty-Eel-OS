"""Smoke tests: router decisions per rule path + --decision-only."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest
from mai import MaiClient, MaiClientConfig

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "compliance_routed_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


# --- router unit-style ----------------------------------------------------

def test_ocap_tribal_routes_local_only() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    router = main.MockComplianceRouter(cfg["routing"]["rules"])
    decision = router.decide(main.RequestMetadata(
        prompt="x",
        compliance_scopes=["ocap"],
        data_classification="tribal_protected",
    ))
    assert decision.route == "local_only"
    assert "OCAP_REQUIRED" in decision.flags
    assert decision.matched_rule_index == 0


def test_itar_controlled_routes_local_only() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    router = main.MockComplianceRouter(cfg["routing"]["rules"])
    decision = router.decide(main.RequestMetadata(
        prompt="x",
        compliance_scopes=["itar_ear"],
        data_classification="controlled",
    ))
    assert decision.route == "local_only"
    assert "ITAR_EXPORT_CONTROL" in decision.flags


def test_hipaa_phi_routes_local_only() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    router = main.MockComplianceRouter(cfg["routing"]["rules"])
    decision = router.decide(main.RequestMetadata(
        prompt="x", compliance_scopes=["hipaa"], data_classification="phi",
    ))
    assert decision.route == "local_only"
    assert "PHI_PROTECTED" in decision.flags


def test_public_default_routes_local_preferred() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    router = main.MockComplianceRouter(cfg["routing"]["rules"])
    decision = router.decide(main.RequestMetadata(
        prompt="x", compliance_scopes=[], data_classification="public",
    ))
    assert decision.route == "local_preferred"
    assert decision.flags == []


def test_no_matching_rule_denies() -> None:
    main = _load_main()
    router = main.MockComplianceRouter([])
    decision = router.decide(main.RequestMetadata(
        prompt="x", compliance_scopes=["ocap"], data_classification="phi",
    ))
    assert decision.is_denied()
    assert "NO_MATCHING_RULE" in decision.flags


# --- entry-point smoke ----------------------------------------------------

def test_decision_only_does_not_call_sdk(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    calls = {"made": 0}

    def explode(_cfg: MaiClientConfig) -> MaiClient:
        calls["made"] += 1
        raise AssertionError("should not be called in --decision-only mode")

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", explode)

    rc = main.run("x", config_path=APP_ROOT / "config.toml",
                  scopes=["ocap"], classification="tribal_protected",
                  decision_only=True)
    assert rc == 0
    assert calls["made"] == 0
    err = capsys.readouterr().err
    assert "local_only" in err


def test_deny_blocks_dispatch_with_exit_code(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    cfg = tmp_path / "cfg.toml"
    # empty rules list -> always denies
    cfg.write_text("[routing]\nrules = []\n[generation]\nchat_model = 'c'\n")

    def explode(_cfg: MaiClientConfig) -> MaiClient:
        raise AssertionError("should not dispatch on deny")

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", explode)

    rc = main.run("x", config_path=cfg)
    assert rc == 4
    err = capsys.readouterr().err
    assert "refusing to dispatch" in err
