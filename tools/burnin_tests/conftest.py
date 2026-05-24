"""SHIP-14 burn-in test fixtures.

Centralises the repo-root path discovery + the posix_only marker so the
per-test files stay focused on their own assertions.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS_DIR = REPO_ROOT / "scripts"

BASH_DRIVER = SCRIPTS_DIR / "burn-in-72h.sh"
PS1_DRIVER = SCRIPTS_DIR / "burn-in-72h.ps1"
SIGNER = SCRIPTS_DIR / "burn-in-report-sign.py"
README = SCRIPTS_DIR / "burn-in-72h-README.md"

posix_only = pytest.mark.skipif(
    sys.platform == "win32",
    reason="bash execution-driven test; runs on the Linux release runner",
)


@pytest.fixture
def repo_root() -> Path:
    return REPO_ROOT


@pytest.fixture
def bash_driver() -> Path:
    return BASH_DRIVER


@pytest.fixture
def ps1_driver() -> Path:
    return PS1_DRIVER


@pytest.fixture
def signer() -> Path:
    return SIGNER
