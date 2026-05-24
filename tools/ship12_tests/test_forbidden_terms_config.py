"""SHIP-12 schema checks for config/forbidden-terms.toml.

Independent of the scanner: this suite asserts the on-disk allowlist file
captures the canonical SHIP-12 forbidden symbols, points at real files,
and never silently grandfathers something outside the production crate
roots.
"""

from __future__ import annotations

import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CONFIG = REPO_ROOT / "config" / "forbidden-terms.toml"

REQUIRED_TERMS = {
    "StubVault",
    "MemoryAuditWriter",
    "AcceptAllBundleVerifier",
    "NullSealer",
    "dashboard-dev",
    "allow_demo_defaults",
}


def _load() -> dict:
    _toml = tomllib
    return _toml.loads(CONFIG.read_text(encoding="utf-8"))


def test_config_exists() -> None:
    assert CONFIG.exists(), "config/forbidden-terms.toml is required"


def test_config_parses() -> None:
    _load()


def test_scan_section_present() -> None:
    cfg = _load()
    scan = cfg.get("scan", {})
    assert isinstance(scan.get("roots"), list)
    assert scan["roots"]
    assert isinstance(scan.get("extensions"), list)
    assert scan["extensions"]


def test_scan_roots_exist() -> None:
    cfg = _load()
    for root in cfg["scan"]["roots"]:
        path = REPO_ROOT / root
        assert path.exists(), f"scan root does not exist: {root}"
        assert path.is_dir(), f"scan root is not a directory: {root}"


def test_required_terms_listed() -> None:
    cfg = _load()
    names = {t["name"] for t in cfg["term"]}
    missing = REQUIRED_TERMS - names
    assert not missing, f"missing forbidden terms: {sorted(missing)}"


def test_each_term_has_metadata() -> None:
    cfg = _load()
    for t in cfg["term"]:
        assert t.get("name"), "term name is required"
        assert isinstance(t.get("allowed_paths"), list)
        assert t.get("carried_forward"), (
            f"term {t['name']!r} must declare carried_forward "
            "(the session that will shrink its allowlist to empty)"
        )


def test_each_term_carried_forward_is_a_ship_session() -> None:
    cfg = _load()
    for t in cfg["term"]:
        cf = t["carried_forward"]
        assert cf.startswith("SHIP-"), (
            f"term {t['name']!r} carried_forward must reference a SHIP session, "
            f"got {cf!r}"
        )


def test_every_allowed_path_exists_on_disk() -> None:
    cfg = _load()
    missing: list[str] = []
    for t in cfg["term"]:
        for p in t["allowed_paths"]:
            if not (REPO_ROOT / p).exists():
                missing.append(f"{t['name']} -> {p}")
    assert not missing, (
        "allowed_paths reference files that do not exist: "
        + ", ".join(missing)
    )


def test_every_allowed_path_is_within_a_scan_root() -> None:
    cfg = _load()
    roots = [Path(r).as_posix().rstrip("/") + "/" for r in cfg["scan"]["roots"]]
    bad: list[str] = []
    for t in cfg["term"]:
        for p in t["allowed_paths"]:
            posix = Path(p).as_posix()
            if not any(posix.startswith(r) for r in roots):
                bad.append(f"{t['name']} -> {p}")
    assert not bad, (
        "allowed_paths must live under a [scan].roots entry: "
        + ", ".join(bad)
    )


def test_term_names_are_unique() -> None:
    cfg = _load()
    names = [t["name"] for t in cfg["term"]]
    assert len(names) == len(set(names)), "duplicate term names in config"


def test_term_allowlists_cover_current_tree_uses() -> None:
    """Sanity: scanner is expected to pass on main. If this test fails the
    allowlist has drifted out of sync with the actual production tree."""
    cfg = _load()
    for t in cfg["term"]:
        name = t["name"]
        allowed = set(t["allowed_paths"])
        for root in cfg["scan"]["roots"]:
            for path in (REPO_ROOT / root).rglob("*"):
                if not path.is_file():
                    continue
                if path.suffix not in set(cfg["scan"]["extensions"]):
                    continue
                rel = path.relative_to(REPO_ROOT).as_posix()
                try:
                    text = path.read_text(encoding="utf-8", errors="replace")
                except OSError:
                    continue
                if name in text and rel not in allowed:
                    raise AssertionError(
                        f"{name} appears in {rel} but {rel} is not in the "
                        f"term's allowed_paths. Either add it (with rationale) "
                        "or remove the use."
                    )
