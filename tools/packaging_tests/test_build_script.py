"""SHIP-08 acceptance: scripts/build-package.{sh,ps1} structure checks.

Both scripts must produce the same staging layout under
build/package-staging/ so downstream tools (dpkg-buildpackage, the
acceptance tests, the operator runbook) see the same tree regardless of
the build host.

This file checks the shapes statically. Actually running the scripts on
Windows would call out to cargo and python, which exceeds the scope of
a unit test. The Linux burn-in CI job in SHIP-14 will execute the .sh
end-to-end.
"""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SH = REPO_ROOT / "scripts" / "build-package.sh"
PS = REPO_ROOT / "scripts" / "build-package.ps1"


def _read(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def test_build_script_present_on_both_platforms() -> None:
    assert SH.is_file(), "scripts/build-package.sh missing"
    assert PS.is_file(), "scripts/build-package.ps1 missing"


def test_sh_has_strict_mode() -> None:
    body = _read(SH)
    assert "set -euo pipefail" in body


def test_sh_stages_required_directories() -> None:
    body = _read(SH)
    for path in (
        "usr/bin",
        "usr/lib/mai/adapters",
        "usr/lib/mai/compliance-dashboard",
        "usr/lib/mai/scripts",
        "usr/share/doc/mai",
        "lib/systemd/system",
        "etc/mai/policies",
        "etc/mai/trust-anchors",
        "DEBIAN",
    ):
        assert path in body, f"build-package.sh missing staging path {path}"


def test_sh_installs_each_systemd_unit() -> None:
    body = _read(SH)
    for unit in (
        "mai-api.service",
        "mai-dashboard.service",
        "mai-adapter-manager.service",
        "mai-healthcheck.service",
        "mai-healthcheck.timer",
    ):
        assert unit in body, f"build-package.sh does not install {unit}"


def test_sh_runs_production_guard_against_staged_profile() -> None:
    body = _read(SH)
    assert "mai-api" in body and "validate" in body and "profile.toml" in body, (
        "build-package.sh must run `mai-api validate` against the staged profile"
    )


def test_sh_records_build_metadata() -> None:
    body = _read(SH)
    assert "PACKAGE_BUILD_INFO" in body
    for field in ("git_commit", "build_time", "version", "profile=ship"):
        assert field in body, f"PACKAGE_BUILD_INFO missing field {field}"


def test_ps_mirrors_sh_layout() -> None:
    body = _read(PS)
    for path in (
        "usr/bin",
        "usr/lib/mai/adapters",
        "usr/lib/mai/compliance-dashboard",
        "lib/systemd/system",
        "etc/mai/policies",
        "etc/mai/trust-anchors",
        "DEBIAN",
    ):
        assert path in body, f"build-package.ps1 missing staging path {path}"


def test_ps_records_build_metadata() -> None:
    body = _read(PS)
    assert "PACKAGE_BUILD_INFO" in body


def test_sh_refuses_to_proceed_on_validator_failure() -> None:
    body = _read(SH)
    # The validator block must exit 2 on failure - this is the documented
    # contract for downstream CI to differentiate validator failure from
    # other build errors.
    assert "die " in body
    assert "production guard rejected" in body
    assert " 2" in body
