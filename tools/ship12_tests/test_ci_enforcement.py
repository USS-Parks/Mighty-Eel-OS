"""SHIP-12 enforcement checks on the always-on CI workflow + pyproject.

The point of SHIP-12 is to remove the `continue-on-error: true` escape
hatch from the mypy step in `.github/workflows/ci.yml`, and to ensure
the per-package overrides that grandfather adapter typing debt point at
SHIP-16 cleanup. These tests freeze that intent.
"""

from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
CI_YML = REPO_ROOT / ".github" / "workflows" / "ci.yml"
PYPROJECT = REPO_ROOT / "pyproject.toml"


@pytest.fixture(scope="module")
def ci_yaml() -> dict:
    yaml = pytest.importorskip("yaml")
    return yaml.safe_load(CI_YML.read_text(encoding="utf-8"))


@pytest.fixture(scope="module")
def pyproject() -> dict:
    _toml = tomllib
    return _toml.loads(PYPROJECT.read_text(encoding="utf-8"))


def _python_check_job(ci_yaml: dict) -> dict:
    return ci_yaml["jobs"]["python-check"]


def test_ci_yml_exists() -> None:
    assert CI_YML.exists()


def test_python_check_job_present(ci_yaml: dict) -> None:
    assert "python-check" in ci_yaml["jobs"]


def test_mypy_step_no_longer_continues_on_error(ci_yaml: dict) -> None:
    job = _python_check_job(ci_yaml)
    mypy_steps = [
        s for s in job["steps"] if "mypy" in (s.get("name", "").lower())
    ]
    assert mypy_steps, "expected at least one mypy step in python-check"
    for step in mypy_steps:
        assert not step.get("continue-on-error", False), (
            f"SHIP-12 forbids continue-on-error on mypy step "
            f"{step.get('name')!r}"
        )


def test_mypy_runs_strict_on_sdk_in_ci(ci_yaml: dict) -> None:
    job = _python_check_job(ci_yaml)
    runs = "\n".join(s.get("run", "") for s in job["steps"])
    assert "mypy --strict mai-sdk-python/src" in runs, (
        "ci.yml python-check must enforce mypy --strict on mai-sdk-python/src/"
    )


def test_mypy_runs_on_adapters_in_ci(ci_yaml: dict) -> None:
    job = _python_check_job(ci_yaml)
    runs = "\n".join(s.get("run", "") for s in job["steps"])
    assert "mypy adapters/" in runs, (
        "ci.yml python-check must enforce mypy on adapters/ (overrides apply)"
    )


def test_pyproject_has_strict_mypy(pyproject: dict) -> None:
    mypy_cfg = pyproject.get("tool", {}).get("mypy", {})
    assert mypy_cfg.get("strict") is True, (
        "[tool.mypy] strict must remain true at the project level"
    )


def test_pyproject_has_ship12_override_for_adapters(pyproject: dict) -> None:
    overrides = pyproject.get("tool", {}).get("mypy", {}).get("overrides", [])
    assert overrides, "SHIP-12 requires a per-module override block"
    adapter_blocks = [
        o for o in overrides
        if "adapters.*" in (o.get("module") or [])
        or o.get("module") == "adapters.*"
    ]
    assert adapter_blocks, "no override block targeting adapters.*"
    block = adapter_blocks[0]
    assert isinstance(block.get("disable_error_code"), list)
    assert block["disable_error_code"], (
        "adapter override must explicitly list the grandfathered error codes"
    )


def test_pyproject_adapter_override_carries_required_codes(pyproject: dict) -> None:
    overrides = pyproject["tool"]["mypy"]["overrides"]
    adapter_blocks = [
        o for o in overrides
        if "adapters.*" in (o.get("module") or [])
    ]
    block = adapter_blocks[0]
    codes = set(block["disable_error_code"])
    # If any of these get dropped from the override list, the adapter tree
    # will start failing mypy and the regression will fail closed here too.
    required = {"no-untyped-def", "no-any-return", "union-attr"}
    missing = required - codes
    assert not missing, f"adapter override missing required codes: {missing}"


def test_ship_validation_workflow_lives_alongside_ci(ci_yaml: dict) -> None:
    sv = REPO_ROOT / ".github" / "workflows" / "ship-validation.yml"
    assert sv.exists(), "SHIP-12 workflow must coexist with ci.yml"


def test_ci_yml_did_not_steal_ship_validation_jobs(ci_yaml: dict) -> None:
    """SHIP-12 keeps the new gates in a separate workflow file (parity with
    SHIP-13 gpu-release.yml). ci.yml should not have absorbed them."""
    forbidden = {
        "forbidden-term-scan",
        "ship-validator",
        "mai-admin-backup",
        "package-build-validate",
    }
    overlap = forbidden & set(ci_yaml["jobs"].keys())
    assert not overlap, (
        f"jobs {sorted(overlap)} belong in ship-validation.yml, not ci.yml"
    )
