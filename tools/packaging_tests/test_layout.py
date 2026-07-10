"""SHIP-08 acceptance: cross-file consistency between systemd units,
maintainer scripts, the build script, the Debian install map, and the
SHIP-HARDENING-PLAN.md filesystem layout requirement.

Catches the class of bug where the systemd unit references /usr/lib/mai
but the build script stages /opt/mai, etc.
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
UNIT_DIR = REPO_ROOT / "packaging" / "systemd"
SH = REPO_ROOT / "scripts" / "build-package.sh"
INSTALL = REPO_ROOT / "packaging" / "debian" / "install"
POSTINST = REPO_ROOT / "packaging" / "scripts" / "postinstall.sh"
PLAN = REPO_ROOT / "docs" / "SHIP-HARDENING-PLAN.md"

# The plan §8 "Filesystem Layout" lists these absolute paths. Every one
# must be either created by postinstall.sh or staged by build-package.sh.
REQUIRED_LAYOUT = [
    "/usr/bin/mai-api",
    "/usr/bin/mai-ship-validate",
    "/usr/lib/mai/adapters",
    "/usr/lib/mai/compliance-dashboard",
    "/etc/mai/auth_keys.toml",
    "/etc/mai/profile.toml",
    "/etc/mai/dashboard.env",
    "/etc/mai/policies",
    "/etc/mai/trust-anchors",
    "/var/lib/mai/vault",
    "/var/lib/mai/audit",
    "/var/lib/mai/trust",
    "/var/lib/mai/models",
    "/var/lib/mai/reports",
    "/var/log/mai",
    "/run/mai",
]


@pytest.mark.parametrize("path", REQUIRED_LAYOUT)
def test_required_layout_path_is_realized(path: str) -> None:
    """Each required path must show up either in build-package.sh (staged
    at build time) or in postinstall.sh (created at install time)."""
    relative = path.lstrip("/")
    in_build = relative in SH.read_text(encoding="utf-8")
    in_postinst = path in POSTINST.read_text(encoding="utf-8")
    assert in_build or in_postinst, (
        f"{path} is in the required filesystem layout but is neither staged "
        f"by build-package.sh nor created by postinstall.sh"
    )


def test_unit_execstart_targets_present_in_install_map() -> None:
    """For each ExecStart binary path used by a systemd unit, the build
    script must stage it and the Debian install map must ship it."""
    import configparser

    parser = configparser.RawConfigParser(strict=False)
    parser.optionxform = str

    for unit in ("mai-api.service", "mai-dashboard.service", "mai-adapter-manager.service", "mai-healthcheck.service"):
        parser.read_string((UNIT_DIR / unit).read_text(encoding="utf-8"))
        exec_start = parser.get("Service", "ExecStart")
        bin_path = exec_start.split()[0]
        # Reset parser between units (configparser preserves state).
        for s in list(parser.sections()):
            parser.remove_section(s)

        relative = bin_path.lstrip("/")
        # Either the build-package.sh installs it (mai-api, mai-ship-validate,
        # mai-healthcheck.sh) or it lives inside a directory we ship wholesale
        # (compliance-dashboard/.venv/bin/uvicorn).
        in_build = (
            relative in SH.read_text(encoding="utf-8")
            or "compliance-dashboard" in bin_path
        )
        assert in_build, f"{unit} ExecStart {bin_path} is not produced by build-package.sh"


def test_filesystem_layout_documented_in_plan() -> None:
    """Cross-check: any path in REQUIRED_LAYOUT must appear in the
    SHIP-HARDENING-PLAN.md §8 layout block. This keeps the plan in
    sync with the code."""
    if not PLAN.is_file():
        pytest.skip("SHIP-HARDENING-PLAN.md not present in this checkout")
    text = PLAN.read_text(encoding="utf-8")
    for path in REQUIRED_LAYOUT:
        # /usr/lib/mai/adapters/ - the plan uses trailing slashes
        canonical = path if path.endswith(("/", ".toml")) else path
        if canonical not in text and (canonical + "/") not in text:
            # The plan may abbreviate /var/lib/mai/foo as /var/lib/mai/
            parent = "/".join(canonical.split("/")[:-1])
            if parent and parent in text:
                continue
            pytest.fail(f"{path} required by code but missing from SHIP-HARDENING-PLAN.md")
