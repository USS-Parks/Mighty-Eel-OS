"""SHIP-08 acceptance: every config template the package installs parses.

The build-package script stages `config/production.example.toml` to
`/etc/mai/profile.toml` and `config/auth_keys.toml` to
`/etc/mai/auth_keys.toml`. Both must be parseable TOML at build time;
otherwise the staged profile will fail `mai-api validate` and the
package build will exit non-zero.

This is independent of the production guard's stricter contract checks
(those live in mai-api/src/ship_profile.rs and run in build-package.sh).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

try:
    import tomllib  # py311+
except ModuleNotFoundError:  # py<3.11 / pypy fallback
    import tomli as tomllib  # type: ignore[no-redef]

REPO_ROOT = Path(__file__).resolve().parents[2]
TEMPLATES = [
    REPO_ROOT / "config" / "production.example.toml",
    REPO_ROOT / "config" / "auth_keys.toml",
    REPO_ROOT / "deployment" / "ship" / "profile.toml",
]


@pytest.mark.parametrize("path", TEMPLATES)
def test_config_template_parses(path: Path) -> None:
    if not path.is_file():
        pytest.skip(f"{path} not present in this checkout")
    with path.open("rb") as f:
        tomllib.load(f)


def test_production_template_has_ship_profile_name() -> None:
    path = REPO_ROOT / "config" / "production.example.toml"
    if not path.is_file():
        pytest.skip(f"{path} not present in this checkout")
    with path.open("rb") as f:
        cfg = tomllib.load(f)
    assert cfg.get("profile", {}).get("name") == "ship"
    assert cfg.get("profile", {}).get("mode") == "production"
    assert cfg.get("profile", {}).get("fail_closed") is True
    assert cfg.get("profile", {}).get("allow_demo_defaults") is False


def test_dashboard_logging_json_is_valid_json() -> None:
    """The dashboard-logging.json blob is emitted inline by
    scripts/build-package.sh. Make sure the heredoc parses as JSON."""
    sh = (REPO_ROOT / "scripts" / "build-package.sh").read_text(encoding="utf-8")
    start = sh.index("dashboard-logging.json")
    eof_open = sh.index("<<'EOF'", start)
    eof_close = sh.index("\nEOF\n", eof_open)
    blob = sh[eof_open + len("<<'EOF'"): eof_close].strip()
    parsed = json.loads(blob)
    assert parsed["version"] == 1
    assert "handlers" in parsed
    assert "root" in parsed
