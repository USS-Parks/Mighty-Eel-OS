"""SHIP-08 acceptance: systemd unit files are well-formed and harden the host.

These tests inspect each .service / .timer in packaging/systemd/ as plain
text (no systemd-analyze required, since the repo is cross-platform). We
verify the contract every unit must satisfy for an unattended install:

    * the file parses as the systemd INI subset
    * required sections exist
    * ExecStart points at an absolute path under /usr/bin or /usr/lib/mai
    * hardening flags requested by SHIP-HARDENING-PLAN.md §8 are present
    * Restart policy is set
    * the Install section targets multi-user.target (or timers.target)

If you change the units, expect to update this file too.
"""

from __future__ import annotations

import configparser
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
UNIT_DIR = REPO_ROOT / "packaging" / "systemd"

SERVICE_UNITS = [
    "mai-api.service",
    "mai-dashboard.service",
    "mai-adapter-manager.service",
    "mai-healthcheck.service",
]
TIMER_UNITS = ["mai-healthcheck.timer"]

# Hardening flags every long-running service must declare. The healthcheck
# is a oneshot, so it relaxes a couple of these.
LONG_RUNNING_HARDENING = {
    "NoNewPrivileges": "true",
    "PrivateTmp": "true",
    "ProtectSystem": "strict",
    "ProtectHome": "true",
}


def _read_unit(name: str) -> configparser.RawConfigParser:
    path = UNIT_DIR / name
    text = path.read_text(encoding="utf-8")
    # systemd allows duplicate keys; RawConfigParser does not. For these
    # tests we don't care about duplicate keys, so collapse them.
    parser = configparser.RawConfigParser(strict=False)
    parser.optionxform = str
    parser.read_string(text)
    return parser


@pytest.mark.parametrize("unit", SERVICE_UNITS + TIMER_UNITS)
def test_unit_file_exists(unit: str) -> None:
    assert (UNIT_DIR / unit).is_file(), f"{unit} missing"


@pytest.mark.parametrize("unit", SERVICE_UNITS + TIMER_UNITS)
def test_unit_parses(unit: str) -> None:
    parser = _read_unit(unit)
    assert "Unit" in parser, f"{unit} missing [Unit]"


# Long-running services and timers must declare an [Install] section so
# `systemctl enable` works. mai-healthcheck.service is a oneshot
# triggered by its timer and intentionally omits [Install].
UNITS_NEEDING_INSTALL = [
    "mai-api.service",
    "mai-dashboard.service",
    "mai-adapter-manager.service",
    "mai-healthcheck.timer",
]


@pytest.mark.parametrize("unit", UNITS_NEEDING_INSTALL)
def test_unit_has_install_section(unit: str) -> None:
    parser = _read_unit(unit)
    assert "Install" in parser, f"{unit} missing [Install]"


@pytest.mark.parametrize("unit", SERVICE_UNITS)
def test_service_has_execstart(unit: str) -> None:
    parser = _read_unit(unit)
    assert "Service" in parser, f"{unit} missing [Service]"
    exec_start = parser.get("Service", "ExecStart", fallback=None)
    assert exec_start, f"{unit} missing ExecStart"
    first = exec_start.split()[0]
    assert first.startswith("/"), f"{unit} ExecStart not absolute: {first}"
    assert first.startswith(("/usr/bin/", "/usr/lib/mai/")), (
        f"{unit} ExecStart {first} not under /usr/bin or /usr/lib/mai"
    )


@pytest.mark.parametrize("unit", ["mai-api.service", "mai-dashboard.service", "mai-adapter-manager.service"])
def test_long_running_unit_hardened(unit: str) -> None:
    parser = _read_unit(unit)
    for key, want in LONG_RUNNING_HARDENING.items():
        got = parser.get("Service", key, fallback=None)
        assert got == want, f"{unit}: {key} = {got!r}, want {want!r}"


@pytest.mark.parametrize("unit", ["mai-api.service", "mai-dashboard.service", "mai-adapter-manager.service"])
def test_long_running_unit_restarts(unit: str) -> None:
    parser = _read_unit(unit)
    restart = parser.get("Service", "Restart", fallback=None)
    assert restart == "on-failure", f"{unit} Restart={restart}, want on-failure"


@pytest.mark.parametrize("unit", SERVICE_UNITS)
def test_service_runs_as_mai_user(unit: str) -> None:
    parser = _read_unit(unit)
    assert parser.get("Service", "User", fallback=None) == "mai"
    assert parser.get("Service", "Group", fallback=None) == "mai"


def test_api_unit_runs_ship_validate_before_start() -> None:
    parser = _read_unit("mai-api.service")
    pre = parser.get("Service", "ExecStartPre", fallback=None)
    assert pre, "mai-api.service must run mai-ship-validate before ExecStart"
    assert "mai-ship-validate" in pre


def test_api_unit_loads_profile() -> None:
    parser = _read_unit("mai-api.service")
    exec_start = parser.get("Service", "ExecStart")
    assert "/etc/mai/profile.toml" in exec_start


def test_timer_targets_timers_target() -> None:
    parser = _read_unit("mai-healthcheck.timer")
    assert parser.get("Install", "WantedBy", fallback="") == "timers.target"


def test_timer_schedule_reasonable() -> None:
    parser = _read_unit("mai-healthcheck.timer")
    assert parser.get("Timer", "OnBootSec", fallback="")
    assert parser.get("Timer", "OnUnitActiveSec", fallback="")
    assert parser.get("Timer", "Persistent", fallback="") == "true"


@pytest.mark.parametrize("unit", SERVICE_UNITS + TIMER_UNITS)
def test_unit_documentation_present(unit: str) -> None:
    parser = _read_unit(unit)
    docs = parser.get("Unit", "Documentation", fallback="")
    assert docs, f"{unit} missing Documentation entry"


def test_api_has_readwrite_paths_covering_state_dirs() -> None:
    parser = _read_unit("mai-api.service")
    rw = parser.get("Service", "ReadWritePaths", fallback="")
    for required in ("/var/lib/mai", "/var/log/mai", "/run/mai"):
        assert required in rw, f"mai-api.service missing ReadWritePaths={required}"


def test_dashboard_depends_on_api() -> None:
    parser = _read_unit("mai-dashboard.service")
    requires = parser.get("Unit", "Requires", fallback="")
    assert "mai-api.service" in requires


def test_dashboard_hard_requires_generated_token_file() -> None:
    """The dashboard must load its admin token from the EnvironmentFile
    the postinstall generates — and refuse to start without it (no "-"
    prefix), never falling back to the built-in local-dev default."""
    parser = _read_unit("mai-dashboard.service")
    env_file = parser.get("Service", "EnvironmentFile", fallback=None)
    assert env_file == "/etc/mai/dashboard.env", (
        f"mai-dashboard.service EnvironmentFile = {env_file!r}; must hard-require "
        "/etc/mai/dashboard.env (an optional '-' prefix would fail open)"
    )


def test_no_unit_listens_on_wildcard() -> None:
    for unit in SERVICE_UNITS:
        text = (UNIT_DIR / unit).read_text(encoding="utf-8")
        assert "0.0.0.0" not in text, f"{unit} binds 0.0.0.0; production must stay loopback"  # nosec B104 — test asserts ABSENCE of 0.0.0.0, never binds
