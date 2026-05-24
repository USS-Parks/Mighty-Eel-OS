"""SHIP-14: basic existence + executability + readme checks."""

from __future__ import annotations

import os
import sys

import pytest

from .conftest import BASH_DRIVER, PS1_DRIVER, README, SIGNER, posix_only


def test_bash_driver_exists() -> None:
    assert BASH_DRIVER.exists(), f"missing {BASH_DRIVER}"


def test_ps1_driver_exists() -> None:
    assert PS1_DRIVER.exists(), f"missing {PS1_DRIVER}"


def test_signer_exists() -> None:
    assert SIGNER.exists(), f"missing {SIGNER}"


def test_readme_exists() -> None:
    assert README.exists(), f"missing {README}"


@pytest.mark.skipif(sys.platform == "win32", reason="executable bit not observable on Windows")
def test_bash_driver_is_executable() -> None:
    assert os.access(BASH_DRIVER, os.X_OK), "burn-in-72h.sh must have +x"


def test_bash_driver_has_shebang() -> None:
    head = BASH_DRIVER.read_bytes()[:32]
    assert head.startswith(b"#!/usr/bin/env bash"), "expected /usr/bin/env bash shebang"


def test_ps1_driver_has_cmdletbinding() -> None:
    text = PS1_DRIVER.read_text(encoding="utf-8")
    assert "[CmdletBinding()]" in text
    assert "param(" in text


def test_signer_is_python_module() -> None:
    text = SIGNER.read_text(encoding="utf-8")
    assert 'if __name__ == "__main__":' in text
    assert "def main(" in text


def test_readme_documents_smoke_mode() -> None:
    text = README.read_text(encoding="utf-8")
    assert "--smoke" in text
    assert "-Smoke" in text
    assert "SHIP-14" in text


def test_readme_documents_every_phase_name() -> None:
    text = README.read_text(encoding="utf-8")
    for phase in [
        "preflight",
        "service-start",
        "mixed-workload",
        "policy-triggers",
        "trust-degradation",
        "adapter-restart",
        "backup-during-load",
        "restore-side-env",
        "metrics-capture",
        "ship-validate",
    ]:
        assert phase in text, f"README missing phase {phase}"


def test_readme_lists_exit_codes() -> None:
    text = README.read_text(encoding="utf-8")
    for code in ["0", "1", "2", "3", "4", "5"]:
        assert f"| {code} " in text or f"| {code}    " in text, f"exit code {code} not in README table"


@posix_only
def test_bash_driver_bash_n_clean() -> None:
    import subprocess
    result = subprocess.run(
        ["bash", "-n", str(BASH_DRIVER)],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
