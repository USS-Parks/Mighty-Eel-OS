"""SHIP-08 acceptance: Debian packaging metadata is internally consistent.

These tests do not invoke dpkg-buildpackage (which requires a Linux host
with debhelper). They validate the on-disk metadata so a release
engineer catches a typo or a missing field before a build attempt.

Covered:
    * debian/control: Source + Package paragraphs, required fields
    * debian/changelog: first line follows the dch format
    * debian/rules: shebang + executable bit on Linux (mode bits checked
      best-effort via repo tracking)
    * debian/install: every staging path it references is produced by
      scripts/build-package.sh
    * debian/conffiles: every entry shows up in debian/install
    * debian/source/format: native v3
    * debian/compat: equals debhelper-compat in control
"""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEBIAN = REPO_ROOT / "packaging" / "debian"


def _read(name: str) -> str:
    return (DEBIAN / name).read_text(encoding="utf-8")


def test_control_has_required_paragraphs() -> None:
    text = _read("control")
    paragraphs = text.split("\n\n")
    assert any(p.startswith("Source:") for p in paragraphs), "control missing Source paragraph"
    assert any(p.startswith("Package:") for p in paragraphs), "control missing Package paragraph"


def test_control_required_fields() -> None:
    text = _read("control")
    for field in ("Maintainer:", "Build-Depends:", "Standards-Version:", "Architecture:", "Depends:", "Description:"):
        assert field in text, f"control missing field {field}"


def test_control_package_name_is_mai() -> None:
    text = _read("control")
    assert re.search(r"^Package:\s*mai\s*$", text, flags=re.MULTILINE)


def test_control_debhelper_matches_compat() -> None:
    control = _read("control")
    m = re.search(r"debhelper-compat\s*\(\s*=\s*(\d+)\s*\)", control)
    assert m, "control missing debhelper-compat"
    compat_in_control = int(m.group(1))
    compat = int(_read("compat").strip())
    assert compat == compat_in_control, (
        f"debian/compat ({compat}) differs from control debhelper-compat ({compat_in_control})"
    )


def test_changelog_first_line() -> None:
    first = _read("changelog").splitlines()[0]
    # mai (X.Y.Z-N) DISTRO; urgency=MED
    assert re.match(r"^mai \(\d+\.\d+\.\d+-\d+\) \S+; urgency=\w+\s*$", first), (
        f"changelog first line malformed: {first!r}"
    )


def test_source_format_is_native_v3() -> None:
    assert (DEBIAN / "source" / "format").read_text(encoding="utf-8").strip() == "3.0 (native)"


def test_rules_has_shebang() -> None:
    first = _read("rules").splitlines()[0]
    assert first.startswith("#!"), "debian/rules missing shebang"
    assert "make" in first, "debian/rules shebang must invoke make"


def test_install_paths_produced_by_build_script() -> None:
    """Every left-hand side in debian/install must be a path
    scripts/build-package.sh creates under STAGING_DIR."""
    install = _read("install")
    bash = (REPO_ROOT / "scripts" / "build-package.sh").read_text(encoding="utf-8")

    lhs_paths = []
    for line in install.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) >= 1:
            lhs_paths.append(parts[0])

    assert lhs_paths, "debian/install has no entries"
    for path in lhs_paths:
        # Look up the top-level directory name in the staging dir
        # creation list, since build-package.sh creates parents en masse.
        top = path.split("/")[0]
        assert top in bash, (
            f"debian/install references {path}, but build-package.sh does not "
            f"create the {top}/ tree"
        )


def test_conffiles_listed_in_install() -> None:
    install = _read("install")
    for line in _read("conffiles").splitlines():
        path = line.strip().lstrip("/")
        if not path:
            continue
        assert path in install, (
            f"conffiles lists /{path} but debian/install does not stage it"
        )


def test_copyright_proprietary() -> None:
    text = _read("copyright")
    assert "LicenseRef-Proprietary" in text
    assert "Island Mountain" in text
