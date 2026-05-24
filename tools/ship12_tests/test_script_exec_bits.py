"""SHIP-12 file-mode regression check.

Windows checkouts don't preserve the unix executable bit, so any shell
script that a CI workflow invokes directly (rather than via `bash <path>`
or an interpreter) must have its mode pinned to 100755 in git.

This test scans every workflow under `.github/workflows/` for a step that
invokes `scripts/*.sh` directly, then asserts each referenced script is
recorded as 100755 in the git index.

If this test fails, fix it with:
    git update-index --chmod=+x scripts/<name>.sh

NOT with `chmod +x` on the working tree — that only changes the local
permission, not the staged mode git ships to CI.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS_DIR = REPO_ROOT / ".github" / "workflows"

# A line whose first non-whitespace token is `scripts/<name>.sh` invokes
# the script directly and therefore requires the executable bit. Lines
# like `bash scripts/foo.sh` or `python3 scripts/foo.sh` route through an
# interpreter and do not need the bit set.
DIRECT_INVOCATION = re.compile(
    r"""
    ^\s*                     # optional leading whitespace
    ['"]?                    # optional opening quote
    (scripts/[\w.\-]+\.sh)   # captured script path
    (?:\s|$|['"]|\\)         # whitespace, EOL, closing quote, or line cont
    """,
    re.VERBOSE | re.MULTILINE,
)


def _git_mode(rel_path: str) -> int:
    """Return the staged file mode for rel_path, e.g. 0o100755."""
    out = subprocess.check_output(
        ["git", "ls-files", "--stage", rel_path],
        cwd=REPO_ROOT,
        text=True,
    ).strip()
    if not out:
        raise AssertionError(f"git does not track {rel_path}")
    return int(out.split()[0], 8)


def _directly_invoked_scripts() -> list[tuple[str, str]]:
    """Return sorted [(workflow_filename, script_path), ...] across every
    workflow, covering both single-line `run:` steps and multi-line
    `run: |` blocks."""
    yaml = pytest.importorskip("yaml")
    hits: set[tuple[str, str]] = set()
    for wf in sorted(WORKFLOWS_DIR.glob("*.yml")):
        doc = yaml.safe_load(wf.read_text(encoding="utf-8"))
        for job in (doc.get("jobs") or {}).values():
            for step in job.get("steps", []) or []:
                run = step.get("run")
                if not isinstance(run, str):
                    continue
                for match in DIRECT_INVOCATION.finditer(run):
                    hits.add((wf.name, match.group(1)))
    return sorted(hits)


def test_at_least_one_workflow_invokes_a_shell_script() -> None:
    """Sanity: this whole test family becomes vacuous if nothing is invoked."""
    invocations = _directly_invoked_scripts()
    assert invocations, (
        "no workflow invokes scripts/*.sh directly — either the regex broke "
        "or the gate is no longer needed"
    )


@pytest.mark.parametrize(
    ("workflow", "script"),
    _directly_invoked_scripts(),
    ids=lambda v: v.replace("/", "_") if isinstance(v, str) else str(v),
)
def test_script_is_executable_in_git(workflow: str, script: str) -> None:
    path = REPO_ROOT / script
    assert path.exists(), f"{workflow} invokes missing script: {script}"
    mode = _git_mode(script)
    # 0o100755 is git's "regular executable file" mode. 0o100644 is the
    # Windows-checkout default; that is exactly the bug this test exists
    # to catch.
    assert mode == 0o100755, (
        f"{workflow} invokes {script} directly, but it is staged as "
        f"{oct(mode)} in git (expected 0o100755). "
        f"Fix: git update-index --chmod=+x {script}"
    )


def test_no_shell_script_invoked_directly_without_shebang() -> None:
    """Belt-and-braces: a script invoked directly must also have a shebang."""
    bad: list[str] = []
    for workflow, script in _directly_invoked_scripts():
        path = REPO_ROOT / script
        first = path.read_text(encoding="utf-8").splitlines()[0] if path.exists() else ""
        if not first.startswith("#!"):
            bad.append(f"{workflow} -> {script}")
    assert not bad, (
        "scripts invoked directly from a workflow must start with a shebang: "
        + ", ".join(bad)
    )
