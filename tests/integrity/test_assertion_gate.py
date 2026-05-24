"""Pytest assertion-count floor gate (W5 / J-10).

Walks every `test_*.py` under `adapters/` and `tests/` and fails when
any file has fewer than 3 assertions. Codifies the floor John flagged
via GitDoctor TST-004 and J-09 enforced manually for the llamacpp /
exllamav2 adapter tests — now it is a hard CI check that catches
regressions on any future test file.

Counting rule (must mirror the J-09 commit message verifier so the
gate and the human-readable count agree):
- A line is "an assertion" iff, after leading whitespace, it starts
  with `assert ` (or `assert(`) or with `with pytest.raises`.
- `__init__.py` and `conftest.py` are excluded — they are not test
  files.
- This gate file is excluded from its own walk so it is not subject
  to recursive self-counting.

The gate runs in normal pytest, not under any opt-in mark — it must
fail closed any time someone lands a thin test file.
"""

from __future__ import annotations

import re
from pathlib import Path

# Resolve mai/ from this file's location: <mai>/tests/integrity/<this>.
ROOT = Path(__file__).resolve().parents[2]
WALK_DIRS = ("adapters", "tests")
EXCLUDE_BASENAMES = frozenset({"__init__.py", "conftest.py"})
SELF = Path(__file__).resolve()

ASSERTION_RE = re.compile(r"^\s*(?:assert[\s(]|with\s+pytest\.raises\b)")


def _count_assertions(path: Path) -> int:
    text = path.read_text(encoding="utf-8")
    return sum(1 for line in text.splitlines() if ASSERTION_RE.match(line))


def _walk_test_files() -> list[Path]:
    found: list[Path] = []
    for dirname in WALK_DIRS:
        base = ROOT / dirname
        if not base.exists():
            continue
        for path in base.rglob("test_*.py"):
            if path.name in EXCLUDE_BASENAMES:
                continue
            if path.resolve() == SELF:
                continue
            found.append(path)
    return found


def test_every_test_file_meets_assertion_floor() -> None:
    """No `test_*.py` under adapters/ or tests/ may have <3 assertions."""
    failures: list[tuple[str, int]] = []
    for path in _walk_test_files():
        count = _count_assertions(path)
        if count < 3:
            failures.append((str(path.relative_to(ROOT)).replace("\\", "/"), count))
    assert not failures, (
        f"Files below the 3-assertion floor: {failures}. "
        f"Either add meaningful assertions or delete the file. "
        f"See J-09 / J-10 in mai/docs/dougherty/."
    )


def test_gate_discovers_known_test_files() -> None:
    """Sanity: the walker must find the J-09 adapter tests."""
    files = _walk_test_files()
    rel = {str(p.relative_to(ROOT)).replace("\\", "/") for p in files}
    assert "adapters/llamacpp/tests/test_adapter.py" in rel
    assert "adapters/exllamav2/tests/test_adapter.py" in rel
    assert "adapters/ollama/tests/test_adapter.py" in rel
    assert len(files) >= 5


def test_gate_excludes_itself_and_package_markers() -> None:
    """The gate must not count itself, __init__.py, or conftest.py."""
    files = _walk_test_files()
    resolved = [p.resolve() for p in files]
    assert SELF not in resolved
    names = {p.name for p in files}
    assert "__init__.py" not in names
    assert "conftest.py" not in names


def test_assertion_regex_matches_real_forms() -> None:
    """Pin the regex behaviour against the four assertion shapes that
    actually show up in this tree, so a refactor of `_count_assertions`
    cannot quietly drop a form."""
    assert ASSERTION_RE.match("    assert x == 1")
    assert ASSERTION_RE.match("\tassert(value)")
    assert ASSERTION_RE.match("        with pytest.raises(ValueError):")
    assert ASSERTION_RE.match("assert not failures")
    # And the negative cases must not match.
    assert not ASSERTION_RE.match("# assert this comment is ignored")
    assert not ASSERTION_RE.match("    x = assertion_function()")
