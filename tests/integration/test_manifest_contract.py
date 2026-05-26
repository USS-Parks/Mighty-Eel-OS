from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

pytestmark = pytest.mark.integration

REPO_ROOT = Path(__file__).resolve().parents[2]


def test_required_pytest_markers_are_declared() -> None:
    pyproject = REPO_ROOT / "pyproject.toml"
    data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
    markers = data["tool"]["pytest"]["ini_options"]["markers"]
    joined = "\n".join(markers)
    for required in ("integration:", "e2e:", "live_backend:"):
        assert required in joined
    assert len(markers) >= 3


def test_e2e_suite_is_discoverable() -> None:
    e2e_smoke = REPO_ROOT / "tests" / "e2e" / "test_compliance_smoke.py"
    text = e2e_smoke.read_text(encoding="utf-8")
    assert "pytestmark = pytest.mark.e2e" in text
