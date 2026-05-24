"""SHIP-13 functional tests for bench_compare.py gate subcommand.

Drives the gate end-to-end against synthetic stored-run fixtures and
asserts the documented exit codes:

  0  pass
  1  regression beyond regression_pct
  2  per-iter latency exceeded max_us
  3  required benchmark missing from latest run
  4  unknown benchmark in latest run (only when fail_on_unknown)
  5  thresholds file missing or malformed
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path
from textwrap import dedent

REPO_ROOT = Path(__file__).resolve().parents[2]
BENCH_COMPARE = REPO_ROOT / "tests" / "benchmarks" / "bench_compare.py"
RESULTS_DIR = REPO_ROOT / "tests" / "benchmarks" / "results"


def _run_gate(
    tmp_path: Path,
    thresholds_text: str,
    runs: list[dict],
    extra_args: list[str] | None = None,
) -> subprocess.CompletedProcess:
    """Stage the fixture runs into the real results dir + invoke gate."""
    thresholds_path = tmp_path / "thresholds.toml"
    thresholds_path.write_text(thresholds_text, encoding="utf-8")

    # Back up any existing results so we don't trample real data.
    backup_dir = tmp_path / "_backup"
    if RESULTS_DIR.exists():
        shutil.copytree(RESULTS_DIR, backup_dir)
        shutil.rmtree(RESULTS_DIR)
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    try:
        for i, run in enumerate(runs):
            ts = f"2026010{i + 1}T000000Z"
            payload = dict(run)
            payload.setdefault("timestamp", ts)
            payload.setdefault("git_commit", f"commit{i:040x}"[:40])
            payload.setdefault(
                "summary",
                {
                    "total": len(run["results"]),
                    "passed": sum(1 for r in run["results"] if r.get("passed", True)),
                    "failed": sum(1 for r in run["results"] if not r.get("passed", True)),
                },
            )
            (RESULTS_DIR / f"bench_{payload['timestamp']}.json").write_text(
                json.dumps(payload), encoding="utf-8"
            )

        argv = [
            sys.executable,
            str(BENCH_COMPARE),
            "gate",
            "--thresholds",
            str(thresholds_path),
        ]
        if extra_args:
            argv.extend(extra_args)
        return subprocess.run(argv, capture_output=True, text=True, cwd=str(REPO_ROOT))
    finally:
        shutil.rmtree(RESULTS_DIR, ignore_errors=True)
        if backup_dir.exists():
            shutil.copytree(backup_dir, RESULTS_DIR)


BASIC_THRESHOLDS = dedent(
    """
    [policy]
    regression_pct = 20
    allow_zero_target = true
    fail_on_missing = true
    fail_on_unknown = false

    [[benchmark]]
    name = "alpha"
    required = true
    max_us = 1000
    min_us = 0
    description = "alpha bench"

    [[benchmark]]
    name = "beta"
    required = true
    max_us = 500
    min_us = 0
    description = "beta bench"
    """
).strip()


def _result(name: str, per_iter_us: int) -> dict:
    return {
        "name": name,
        "passed": True,
        "per_iter_us": per_iter_us,
        "target_us": 0,
        "iterations": 1000,
        "total_duration_us": per_iter_us * 1000,
    }


def test_gate_passes_when_all_within_threshold(tmp_path: Path) -> None:
    runs = [{"results": [_result("alpha", 100), _result("beta", 100)]}]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "GATE PASS" in result.stdout


def test_gate_fails_on_threshold_violation(tmp_path: Path) -> None:
    runs = [{"results": [_result("alpha", 100), _result("beta", 9999)]}]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 2, result.stdout + result.stderr
    assert "VIOLATION" in result.stdout
    assert "beta" in result.stdout


def test_gate_fails_on_missing_required_benchmark(tmp_path: Path) -> None:
    runs = [{"results": [_result("alpha", 100)]}]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 3, result.stdout + result.stderr
    assert "missing" in result.stdout.lower()
    assert "beta" in result.stdout


def test_gate_fails_on_regression(tmp_path: Path) -> None:
    runs = [
        {"results": [_result("alpha", 100), _result("beta", 100)]},
        {"results": [_result("alpha", 200), _result("beta", 100)]},  # 100% slower
    ]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION" in result.stdout
    assert "alpha" in result.stdout


def test_gate_tolerates_minor_jitter(tmp_path: Path) -> None:
    runs = [
        {"results": [_result("alpha", 100), _result("beta", 100)]},
        {"results": [_result("alpha", 110), _result("beta", 100)]},  # +10%, under 20%
    ]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 0, result.stdout + result.stderr


def test_gate_regression_pct_override(tmp_path: Path) -> None:
    runs = [
        {"results": [_result("alpha", 100), _result("beta", 100)]},
        {"results": [_result("alpha", 115), _result("beta", 100)]},  # +15%
    ]
    # Default 20% passes; override to 10% should fail.
    result = _run_gate(
        tmp_path, BASIC_THRESHOLDS, runs, extra_args=["--regression-pct", "10"]
    )
    assert result.returncode == 1, result.stdout + result.stderr


def test_gate_config_error_on_missing_thresholds(tmp_path: Path) -> None:
    [{"results": [_result("alpha", 100), _result("beta", 100)]}]
    argv = [
        sys.executable,
        str(BENCH_COMPARE),
        "gate",
        "--thresholds",
        str(tmp_path / "does-not-exist.toml"),
    ]
    result = subprocess.run(argv, capture_output=True, text=True, cwd=str(REPO_ROOT))
    assert result.returncode == 5, result.stdout + result.stderr
    assert "CONFIG" in result.stdout.upper() or "CONFIG" in result.stderr.upper()


def test_gate_config_error_on_empty_thresholds(tmp_path: Path) -> None:
    runs = [{"results": [_result("alpha", 100), _result("beta", 100)]}]
    empty_thresholds = "[policy]\nregression_pct = 20\n"
    result = _run_gate(tmp_path, empty_thresholds, runs)
    assert result.returncode == 5, result.stdout + result.stderr


def test_gate_unknown_benchmark_ignored_by_default(tmp_path: Path) -> None:
    runs = [
        {
            "results": [
                _result("alpha", 100),
                _result("beta", 100),
                _result("unexpected_bench", 50),
            ]
        }
    ]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "unexpected_bench" in result.stdout  # listed in unknown section


def test_gate_unknown_benchmark_fails_when_strict(tmp_path: Path) -> None:
    strict_thresholds = BASIC_THRESHOLDS.replace(
        "fail_on_unknown = false", "fail_on_unknown = true"
    )
    runs = [
        {
            "results": [
                _result("alpha", 100),
                _result("beta", 100),
                _result("unexpected_bench", 50),
            ]
        }
    ]
    result = _run_gate(tmp_path, strict_thresholds, runs)
    assert result.returncode == 4, result.stdout + result.stderr


def test_gate_emits_json_report(tmp_path: Path) -> None:
    runs = [{"results": [_result("alpha", 100), _result("beta", 100)]}]
    json_path = tmp_path / "report.json"
    result = _run_gate(
        tmp_path, BASIC_THRESHOLDS, runs, extra_args=["--json", str(json_path)]
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert json_path.exists()
    report = json.loads(json_path.read_text(encoding="utf-8"))
    assert report["regression_pct_limit"] == 20.0
    assert {entry["name"] for entry in report["checked"]} == {"alpha", "beta"}
    assert report["missing"] == []
    assert report["violations"] == []


def test_gate_no_previous_run_skips_regression_check(tmp_path: Path) -> None:
    # First-ever release: no historical baseline; gate must not fail
    # on regression-axis (only thresholds + missing).
    runs = [{"results": [_result("alpha", 100), _result("beta", 100)]}]
    result = _run_gate(tmp_path, BASIC_THRESHOLDS, runs)
    assert result.returncode == 0, result.stdout + result.stderr
