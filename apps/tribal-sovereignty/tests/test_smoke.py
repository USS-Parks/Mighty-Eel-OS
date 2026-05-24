"""Smoke tests: claim construction, guards, corpus load, --dry-run."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest
from mai import MaiClient, MaiClientConfig

APP_ROOT = Path(__file__).resolve().parents[1]


def _load_main():
    spec = importlib.util.spec_from_file_location(
        "tribal_sovereignty_main", APP_ROOT / "main.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_claim_from_config_populates_all_fields() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    claim = main.claim_from_config(cfg["trust"])
    assert claim.tenant_id == "nation-of-example"
    assert claim.compliance_scopes == ["ocap"]
    assert claim.allowed_routes == ["local_only"]
    assert "elder" in claim.roles


def test_guard_route_passes_for_allowed_route() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    claim = main.claim_from_config(cfg["trust"])
    # should not raise
    main.guard_route(claim, "local_only")


def test_guard_route_rejects_cloud_route() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    claim = main.claim_from_config(cfg["trust"])
    with pytest.raises(main.SovereigntyViolation) as ei:
        main.guard_route(claim, "cloud_allowed")
    assert "cloud_allowed" in str(ei.value)


def test_guard_model_rejects_non_allowlisted_model() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    claim = main.claim_from_config(cfg["trust"])
    with pytest.raises(main.SovereigntyViolation):
        main.guard_model(claim, "gpt-4o")
    main.guard_model(claim, "qwen3-14b:Q4_K_M")  # OK


def test_guard_model_passes_when_allowlist_empty() -> None:
    main = _load_main()
    cfg = main.load_app_config(APP_ROOT / "config.toml")
    trust = dict(cfg["trust"])
    trust["allowed_models"] = []
    claim = main.claim_from_config(trust)
    main.guard_model(claim, "any-model-id")  # OK


def test_load_corpus_reads_protected_dir() -> None:
    main = _load_main()
    corpus = main.load_corpus(APP_ROOT / "protected_data")
    assert len(corpus) >= 1
    names = [name for name, _ in corpus]
    assert "story_excerpt.md" in names


def test_dry_run_does_not_call_sdk(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    def explode(_cfg: MaiClientConfig) -> MaiClient:
        raise AssertionError("should not call SDK in --dry-run")

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", explode)

    rc = main.run("hi", config_path=APP_ROOT / "config.toml", dry_run=True)
    assert rc == 0
    out = capsys.readouterr().out
    assert "local_only" in out
    assert "story_excerpt.md" in out


def test_cloud_intent_with_local_only_claim_refuses(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    def explode(_cfg: MaiClientConfig) -> MaiClient:
        raise AssertionError("should not dispatch on sovereignty violation")

    main = _load_main()
    monkeypatch.setattr(main, "_make_client", explode)

    rc = main.run("hi", config_path=APP_ROOT / "config.toml",
                  intended_route="cloud_allowed", dry_run=True)
    err = capsys.readouterr().err
    assert rc == 4
    assert "sovereignty violation" in err
