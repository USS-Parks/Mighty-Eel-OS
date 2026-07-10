"""SHIP-08 acceptance: maintainer scripts are well-formed and idempotent.

Each of packaging/scripts/{preinstall,postinstall,preremove,postremove}.sh
runs under the host shell at package transition time. They must:

    * start with a POSIX-safe shebang
    * set strict error handling (set -eu)
    * be idempotent (callable repeatedly without breakage)
    * never delete customer state outside `purge`
    * delegate consistently to systemctl when present

Tests are static analysis only - actually running them would require a
Linux host with systemd, an `mai` user, and root.
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "packaging" / "scripts"

MAINTAINER = ["preinstall.sh", "postinstall.sh", "preremove.sh", "postremove.sh"]
HELPERS = ["mai-ship-validate.sh", "mai-healthcheck.sh"]


def _read(name: str) -> str:
    return (SCRIPTS / name).read_text(encoding="utf-8")


@pytest.mark.parametrize("name", MAINTAINER + HELPERS)
def test_script_exists(name: str) -> None:
    assert (SCRIPTS / name).is_file(), f"{name} missing"


@pytest.mark.parametrize("name", MAINTAINER + HELPERS)
def test_script_has_posix_shebang(name: str) -> None:
    first = _read(name).splitlines()[0]
    assert first in ("#!/bin/sh", "#!/usr/bin/env sh", "#!/usr/bin/env bash"), (
        f"{name} shebang {first!r} not POSIX-safe"
    )


@pytest.mark.parametrize("name", MAINTAINER + HELPERS)
def test_script_strict_mode(name: str) -> None:
    body = _read(name)
    assert "set -eu" in body or "set -euo pipefail" in body, (
        f"{name} missing 'set -eu' (strict error handling)"
    )


@pytest.mark.parametrize("name", MAINTAINER)
def test_maintainer_logs_action(name: str) -> None:
    body = _read(name)
    assert "ACTION=" in body, f"{name} must capture the dpkg ACTION arg"
    assert 'printf "[%s' in body, f"{name} should log with PKG_NAME prefix"


def test_postinstall_creates_mai_user() -> None:
    body = _read("postinstall.sh")
    assert "adduser" in body
    assert "addgroup" in body
    assert "/var/lib/mai" in body
    assert "/var/log/mai" in body


def test_postinstall_does_not_auto_start_services() -> None:
    """Auto-start would break the operator workflow that requires
    /etc/mai/auth_keys.toml to be seeded first.

    We only flag invocations on bare code lines - comments and the
    operator next-steps heredoc are allowed to mention the commands.
    """
    code_lines = []
    in_heredoc = False
    for raw in _read("postinstall.sh").splitlines():
        stripped = raw.lstrip()
        if "cat <<EOF" in raw:
            in_heredoc = True
            continue
        if in_heredoc:
            if raw.strip() == "EOF":
                in_heredoc = False
            continue
        if stripped.startswith("#"):
            continue
        code_lines.append(raw)
    code = "\n".join(code_lines)
    assert "systemctl start" not in code, "postinstall must not auto-start services"
    assert "systemctl enable" not in code, "postinstall must not auto-enable services"


def test_postinstall_generates_dashboard_token() -> None:
    """The dashboard admin token is generated at install so no
    deployment ever runs on the built-in local-dev default. The
    generation must be idempotent (an existing token survives
    upgrades), sourced from urandom, and locked down to root:mai."""
    body = _read("postinstall.sh")
    assert "/etc/mai/dashboard.env" in body, "postinstall must write the dashboard EnvironmentFile"
    assert "MAI_DASHBOARD_ADMIN_TOKEN" in body
    assert "/dev/urandom" in body, "token must come from a CSPRNG source"
    fn_start = body.index("generate_dashboard_env()")
    fn_body = body[fn_start : body.index("\n}", fn_start)]
    assert '-f "${env_file}"' in fn_body, "generation must be guarded so an existing token survives upgrades"
    assert "chmod 0640" in fn_body, "token file must not be world-readable"


def test_preremove_disables_services() -> None:
    body = _read("preremove.sh")
    assert "systemctl stop" in body
    assert "systemctl disable" in body
    assert "mai-api.service" in body


def test_postremove_preserves_data_by_default() -> None:
    body = _read("postremove.sh")
    # The script must branch on `purge` and only delete state in that branch.
    assert "purge)" in body, "postremove.sh missing purge branch"
    # The non-purge branch must NOT delete /var/lib/mai.
    remove_branch_start = body.index("remove|upgrade")
    purge_branch_start = body.index("purge)")
    # Find the rm -rf line if present and assert it sits inside the
    # purge branch.
    rm_line = body.find("rm -rf /var/lib/mai")
    assert rm_line != -1, "postremove.sh must include the purge rm command"
    assert rm_line < remove_branch_start or rm_line > purge_branch_start, (
        "rm -rf /var/lib/mai must live in the purge branch, not the default remove path"
    )


def test_postremove_warns_about_purge_in_default_branch() -> None:
    body = _read("postremove.sh")
    assert "preserved" in body
    assert "apt purge mai" in body


def test_mai_ship_validate_wrapper_delegates_to_mai_api() -> None:
    body = _read("mai-ship-validate.sh")
    assert "exec" in body
    assert "validate" in body
    assert "MAI_API_BIN" in body


def test_healthcheck_probes_ready_and_live_endpoints() -> None:
    body = _read("mai-healthcheck.sh")
    assert "/v1/health/ready" in body
    assert "/v1/health/live" in body
