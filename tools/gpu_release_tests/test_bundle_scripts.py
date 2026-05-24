"""SHIP-13 functional + parity tests for the release bundle scripts.

Drives scripts/gpu-release-bundle.sh end-to-end against a synthetic
input tree, then validates the schema of the emitted release-manifest.json
and the integrity of the .tar.gz bundle. Also asserts sh/ps1 parity at
the contract layer (same flags, same manifest fields), without actually
running PowerShell on the CI runner.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tarfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SH_SCRIPT = REPO_ROOT / "scripts" / "gpu-release-bundle.sh"
PS1_SCRIPT = REPO_ROOT / "scripts" / "gpu-release-bundle.ps1"

# The sh script is executed by bash; on Windows pytest runs we skip the
# execution-driven tests (path translation mangles --input/--output and
# stat -c%s is not portable). Static contract tests still run everywhere.
posix_only = pytest.mark.skipif(
    sys.platform == "win32",
    reason="bash execution-driven test; runs on the Linux GPU runner",
)


def _stage_inputs(tmp_path: Path) -> Path:
    src = tmp_path / "input"
    (src / "readiness").mkdir(parents=True)
    (src / "bench").mkdir(parents=True)
    (src / "readiness" / "readiness-report.json").write_text(
        '{"ok": true}', encoding="utf-8"
    )
    (src / "bench" / "gate-report.json").write_text(
        '{"checked": []}', encoding="utf-8"
    )
    (src / "bench" / "bench_2026.json").write_text(
        '{"timestamp": "2026"}', encoding="utf-8"
    )
    return src


def _invoke_sh(tmp_path: Path, args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(SH_SCRIPT), *args],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="bash invocation requires a POSIX shell; sh-only test",
)
def test_sh_script_exists_and_is_executable() -> None:
    assert SH_SCRIPT.exists()
    # On POSIX the exec bit should be set; on Windows we can't observe it,
    # so the skipif above already routes around that.
    assert os.access(SH_SCRIPT, os.X_OK), "sh script should be +x"


@posix_only
def test_sh_script_bash_n_clean() -> None:
    result = subprocess.run(
        ["bash", "-n", str(SH_SCRIPT)],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def test_ps1_script_exists() -> None:
    assert PS1_SCRIPT.exists()


@posix_only
def test_sh_script_rejects_missing_args(tmp_path: Path) -> None:
    result = _invoke_sh(tmp_path, [])
    assert result.returncode == 1


@posix_only
def test_sh_script_rejects_nonexistent_input(tmp_path: Path) -> None:
    result = _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "abc1234567890abc",
            "--input", str(tmp_path / "nope"),
            "--output", str(tmp_path / "out"),
        ],
    )
    assert result.returncode == 2


@posix_only
def test_sh_script_rejects_empty_input(tmp_path: Path) -> None:
    src = tmp_path / "input"
    src.mkdir()
    result = _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "abc1234567890abc",
            "--input", str(src),
            "--output", str(tmp_path / "out"),
        ],
    )
    assert result.returncode == 2


@posix_only
def test_sh_script_assembles_bundle(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    commit = "0123456789abcdef0123456789abcdef01234567"
    result = _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", commit,
            "--input", str(src),
            "--output", str(out),
        ],
    )
    assert result.returncode == 0, result.stdout + result.stderr
    manifest_path = out / "release-manifest.json"
    bundle_path = out / "release-bundle-0123456789ab.tar.gz"
    assert manifest_path.exists()
    assert bundle_path.exists()
    assert (out / "release-bundle-0123456789ab.tar.gz.sha256").exists()


@posix_only
def test_manifest_schema(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    commit = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    _invoke_sh(
        tmp_path,
        [
            "--version", "1.2.3-rc1",
            "--commit", commit,
            "--input", str(src),
            "--output", str(out),
        ],
    )
    manifest = json.loads((out / "release-manifest.json").read_text())
    assert manifest["schema_version"] == 1
    assert manifest["ship_session"] == "SHIP-13"
    assert manifest["release"]["version"] == "1.2.3-rc1"
    assert manifest["release"]["commit"] == commit
    assert manifest["release"]["short_commit"] == "deadbeefdead"
    assert manifest["totals"]["file_count"] == len(manifest["artifacts"])
    assert manifest["totals"]["total_bytes"] == sum(
        a["size_bytes"] for a in manifest["artifacts"]
    )
    assert manifest["signature"] is None
    assert manifest["signature_alg"] is None
    for entry in manifest["artifacts"]:
        assert "path" in entry
        assert "size_bytes" in entry
        assert "sha256" in entry
        assert len(entry["sha256"]) == 64


@posix_only
def test_manifest_artifacts_sorted(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "1234567890abcdef",
            "--input", str(src),
            "--output", str(out),
        ],
    )
    manifest = json.loads((out / "release-manifest.json").read_text())
    paths = [a["path"] for a in manifest["artifacts"]]
    assert paths == sorted(paths), "artifacts must be sorted for determinism"


@posix_only
def test_manifest_sha256_matches_actual_file(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "1234567890abcdef",
            "--input", str(src),
            "--output", str(out),
        ],
    )
    manifest = json.loads((out / "release-manifest.json").read_text())
    for entry in manifest["artifacts"]:
        # The input file lives at src/<entry["path"]>. The script COPIES
        # the manifest into the input tree as `release-manifest.json` —
        # we skip that one since the script computes the sha before
        # injecting the manifest.
        if entry["path"] == "release-manifest.json":
            continue
        actual = hashlib.sha256(
            (src / entry["path"]).read_bytes()
        ).hexdigest()
        assert actual == entry["sha256"], f"{entry['path']}: sha mismatch"


@posix_only
def test_bundle_tarball_opens_and_contains_manifest(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "1234567890abcdef",
            "--input", str(src),
            "--output", str(out),
        ],
    )
    bundle = out / "release-bundle-1234567890ab.tar.gz"
    with tarfile.open(bundle, "r:gz") as tar:
        names = tar.getnames()
        assert any("release-manifest.json" in n for n in names), (
            "tarball must contain release-manifest.json"
        )


@posix_only
def test_bundle_sha256_sidecar_matches_tarball(tmp_path: Path) -> None:
    src = _stage_inputs(tmp_path)
    out = tmp_path / "out"
    _invoke_sh(
        tmp_path,
        [
            "--version", "0.1.0",
            "--commit", "1234567890abcdef",
            "--input", str(src),
            "--output", str(out),
        ],
    )
    bundle = out / "release-bundle-1234567890ab.tar.gz"
    sidecar = bundle.with_suffix(bundle.suffix + ".sha256")
    declared = sidecar.read_text().split()[0]
    actual = hashlib.sha256(bundle.read_bytes()).hexdigest()
    assert declared == actual


# ─── sh / ps1 parity ────────────────────────────────────────────────────


REQUIRED_FLAGS = ["--version", "--commit", "--input", "--output"]
REQUIRED_PS1_PARAMS = ["$Version", "$Commit", "$Input", "$Output"]
REQUIRED_MANIFEST_FIELDS = [
    "schema_version",
    "ship_session",
    "release",
    "totals",
    "signature",
    "signature_alg",
    "artifacts",
]


@pytest.mark.parametrize("flag", REQUIRED_FLAGS)
def test_sh_script_documents_flag(flag: str) -> None:
    text = SH_SCRIPT.read_text(encoding="utf-8")
    assert flag in text


@pytest.mark.parametrize("param", REQUIRED_PS1_PARAMS)
def test_ps1_script_declares_param(param: str) -> None:
    text = PS1_SCRIPT.read_text(encoding="utf-8")
    assert param in text


@pytest.mark.parametrize("field", REQUIRED_MANIFEST_FIELDS)
def test_both_scripts_emit_each_manifest_field(field: str) -> None:
    sh_text = SH_SCRIPT.read_text(encoding="utf-8")
    ps1_text = PS1_SCRIPT.read_text(encoding="utf-8")
    assert field in sh_text, f"sh script missing manifest field: {field}"
    assert field in ps1_text, f"ps1 script missing manifest field: {field}"


def test_both_scripts_use_same_ship_session_marker() -> None:
    assert "SHIP-13" in SH_SCRIPT.read_text(encoding="utf-8")
    assert "SHIP-13" in PS1_SCRIPT.read_text(encoding="utf-8")


def test_both_scripts_use_same_schema_version() -> None:
    sh_text = SH_SCRIPT.read_text(encoding="utf-8")
    ps1_text = PS1_SCRIPT.read_text(encoding="utf-8")
    assert '"schema_version": 1' in sh_text
    assert "schema_version" in ps1_text
    assert "= 1" in ps1_text
