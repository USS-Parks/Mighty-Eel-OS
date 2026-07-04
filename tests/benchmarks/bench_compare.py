#!/usr/bin/env python3
"""MAI Benchmark Result Storage and Comparison Tool.

Stores benchmark results as JSON, compares across runs, and flags regressions.

Usage:
    # Store results from test output:
    cargo test -p mai-adapters --features benchmark -- --nocapture 2>&1 | \
        python3 tests/benchmarks/bench_compare.py store

    # Compare last two runs:
    python3 tests/benchmarks/bench_compare.py compare

    # Show history for a specific benchmark:
    python3 tests/benchmarks/bench_compare.py history throughput_routes_per_sec

    # List all stored runs:
    python3 tests/benchmarks/bench_compare.py list

"""

import json
import re
import sys
from datetime import UTC, datetime
from pathlib import Path

RESULTS_DIR = Path(__file__).parent / "results"
REGRESSION_THRESHOLD = 0.20  # 20% regression triggers warning


def parse_benchmark_output(lines: list[str]) -> list[dict]:
    """Parse benchmark results from cargo test --nocapture output.

    Expected format from report_result():
    [PASS] name : 42us/iter (target: <100us) [1000 iterations in 42000us]
    [FAIL] name : 6000us/iter (target: <5000us) [1000 iterations in 6000000us]
    """
    results = []
    pattern = re.compile(
        r"\[(PASS|FAIL)\]\s+(\S+)\s+:\s+(\d+)us/iter\s+"
        r"\(target:\s+<(\d+)us\)\s+"
        r"\[(\d+)\s+iterations\s+in\s+(\d+)us\]"
    )

    for line in lines:
        match = pattern.search(line)
        if match:
            status, name, per_iter, target, iterations, total = match.groups()
            results.append({
                "name": name,
                "passed": status == "PASS",
                "per_iter_us": int(per_iter),
                "target_us": int(target),
                "iterations": int(iterations),
                "total_duration_us": int(total),
            })

    return results


def store_results(lines: list[str]) -> None:
    """Parse benchmark output and store as timestamped JSON."""
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    results = parse_benchmark_output(lines)
    if not results:
        print("No benchmark results found in input.")
        sys.exit(1)

    timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    run = {
        "timestamp": timestamp,
        "git_commit": _get_git_commit(),
        "results": results,
        "summary": {
            "total": len(results),
            "passed": sum(1 for r in results if r["passed"]),
            "failed": sum(1 for r in results if not r["passed"]),
        },
    }

    filepath = RESULTS_DIR / f"bench_{timestamp}.json"
    with open(filepath, "w") as f:
        json.dump(run, f, indent=2)

    print(f"Stored {len(results)} results to {filepath}")
    for r in results:
        status = "PASS" if r["passed"] else "FAIL"
        print(f"  [{status}] {r['name']}: {r['per_iter_us']}us/iter")


def compare_runs() -> None:
    """Compare the two most recent benchmark runs."""
    runs = _load_all_runs()
    if len(runs) < 2:
        print("Need at least 2 runs to compare. Run benchmarks again.")
        sys.exit(1)

    current = runs[-1]
    previous = runs[-2]

    print(f"Comparing: {current['timestamp']} vs {previous['timestamp']}")
    print(f"  Current commit:  {current.get('git_commit', 'unknown')}")
    print(f"  Previous commit: {previous.get('git_commit', 'unknown')}")
    print()

    prev_map = {r["name"]: r for r in previous["results"]}
    regressions = 0

    for result in current["results"]:
        name = result["name"]
        prev = prev_map.get(name)
        if prev is None:
            print(f"  [NEW] {name}: {result['per_iter_us']}us/iter")
            continue

        curr_us = result["per_iter_us"]
        prev_us = prev["per_iter_us"]

        if prev_us == 0:
            delta_pct = 0.0
        else:
            delta_pct = (curr_us - prev_us) / prev_us

        if delta_pct > REGRESSION_THRESHOLD:
            marker = "REGRESSION"
            regressions += 1
        elif delta_pct < -REGRESSION_THRESHOLD:
            marker = "IMPROVEMENT"
        else:
            marker = "STABLE"

        print(
            f"  [{marker}] {name}: {curr_us}us -> {prev_us}us "
            f"({delta_pct:+.1%})"
        )

    print()
    if regressions > 0:
        print(f"WARNING: {regressions} regression(s) detected (>{REGRESSION_THRESHOLD:.0%} slower)")
    else:
        print("No regressions detected.")


def show_history(bench_name: str) -> None:
    """Show historical performance for a specific benchmark."""
    runs = _load_all_runs()
    print(f"History for: {bench_name}")
    print(f"{'Timestamp':<22} {'us/iter':>10} {'Target':>10} {'Status':>8}")
    print("-" * 54)

    for run in runs:
        for result in run["results"]:
            if result["name"] == bench_name:
                status = "PASS" if result["passed"] else "FAIL"
                print(
                    f"{run['timestamp']:<22} "
                    f"{result['per_iter_us']:>10} "
                    f"{result['target_us']:>10} "
                    f"{status:>8}"
                )


def list_runs() -> None:
    """List all stored benchmark runs."""
    runs = _load_all_runs()
    if not runs:
        print("No benchmark runs stored.")
        return

    print(f"{'Timestamp':<22} {'Commit':<12} {'Total':>6} {'Pass':>6} {'Fail':>6}")
    print("-" * 58)
    for run in runs:
        summary = run.get("summary", {})
        commit = run.get("git_commit", "unknown")[:10]
        print(
            f"{run['timestamp']:<22} "
            f"{commit:<12} "
            f"{summary.get('total', '?'):>6} "
            f"{summary.get('passed', '?'):>6} "
            f"{summary.get('failed', '?'):>6}"
        )


def _load_all_runs() -> list[dict]:
    """Load all benchmark runs sorted by timestamp."""
    if not RESULTS_DIR.exists():
        return []

    runs = []
    for filepath in sorted(RESULTS_DIR.glob("bench_*.json")):
        with open(filepath) as f:
            runs.append(json.load(f))
    return runs


def _get_git_commit() -> str:
    """Get current git commit hash."""
    try:
        import subprocess
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        return result.stdout.strip() if result.returncode == 0 else "unknown"
    except Exception:
        return "unknown"


# ─── SHIP-13: release gate ─────────────────────────────────────────────


GATE_EXIT_PASS = 0
GATE_EXIT_REGRESSION = 1
GATE_EXIT_THRESHOLD = 2
GATE_EXIT_MISSING = 3
GATE_EXIT_UNKNOWN = 4
GATE_EXIT_CONFIG = 5


def _load_thresholds(path: Path) -> dict:
    """Load + lightly validate the thresholds TOML.

    Requires Python 3.11+ (tomllib in stdlib). The repo standardizes on
    3.12 per .github/workflows/gpu-release.yml, so no fallback needed.
    """
    try:
        import tomllib
    except ModuleNotFoundError as exc:  # pragma: no cover
        raise RuntimeError(
            "tomllib not available; require Python >= 3.11"
        ) from exc

    if not path.exists():
        raise FileNotFoundError(f"thresholds file not found: {path}")

    with open(path, "rb") as f:
        data = tomllib.load(f)

    policy = data.get("policy", {}) or {}
    benchmarks = data.get("benchmark", []) or []
    if not isinstance(benchmarks, list) or not benchmarks:
        raise ValueError(
            f"thresholds file {path} has no [[benchmark]] entries"
        )
    for entry in benchmarks:
        if "name" not in entry:
            raise ValueError(f"[[benchmark]] entry missing 'name': {entry}")
        if "max_us" not in entry:
            raise ValueError(
                f"[[benchmark]] '{entry['name']}' missing max_us"
            )

    return {"policy": policy, "benchmarks": benchmarks}


def gate(argv: list[str]) -> int:
    """Apply release thresholds + regression policy to the latest run.

    Exit codes:
      0  pass
      1  regression vs previous run beyond policy.regression_pct
      2  per-iter latency exceeded a declared max_us
      3  required benchmark absent from latest run
      4  unknown benchmark in latest run (only when fail_on_unknown)
      5  thresholds file missing or malformed
    """
    import argparse

    parser = argparse.ArgumentParser(
        prog="bench_compare.py gate",
        description="Enforce SHIP-13 release thresholds against latest run",
    )
    parser.add_argument("--thresholds", required=True, type=Path)
    parser.add_argument(
        "--json",
        type=Path,
        default=None,
        help="Optional path to emit a machine-readable gate report",
    )
    parser.add_argument(
        "--regression-pct",
        type=float,
        default=None,
        help="Override policy.regression_pct (percent, e.g. 15)",
    )
    args = parser.parse_args(argv)

    try:
        thresholds = _load_thresholds(args.thresholds)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"GATE CONFIG ERROR: {exc}")
        return GATE_EXIT_CONFIG

    policy = thresholds["policy"]
    benchmarks = thresholds["benchmarks"]
    regression_pct = (
        args.regression_pct
        if args.regression_pct is not None
        else float(policy.get("regression_pct", 20))
    )
    fail_on_missing = bool(policy.get("fail_on_missing", True))
    fail_on_unknown = bool(policy.get("fail_on_unknown", False))
    allow_zero_target = bool(policy.get("allow_zero_target", True))

    runs = _load_all_runs()
    if not runs:
        print("GATE CONFIG ERROR: no stored runs under tests/benchmarks/results/")
        return GATE_EXIT_CONFIG

    current = runs[-1]
    previous = runs[-2] if len(runs) >= 2 else None
    current_map = {r["name"]: r for r in current["results"]}
    previous_map = {r["name"]: r for r in previous["results"]} if previous else {}
    threshold_names = {b["name"] for b in benchmarks}

    missing: list[str] = []
    violations: list[dict] = []
    regressions: list[dict] = []
    unknown: list[str] = []
    checked: list[dict] = []

    for spec in benchmarks:
        name = spec["name"]
        required = bool(spec.get("required", True))
        max_us = int(spec["max_us"])
        result = current_map.get(name)
        if result is None:
            if required and fail_on_missing:
                missing.append(name)
            continue

        per_iter = int(result["per_iter_us"])
        record = {
            "name": name,
            "per_iter_us": per_iter,
            "max_us": max_us,
            "passed_threshold": True,
            "passed_regression": True,
        }

        if (max_us > 0 or not allow_zero_target) and max_us > 0 and per_iter > max_us:
            record["passed_threshold"] = False
            violations.append({
                "name": name,
                "per_iter_us": per_iter,
                "max_us": max_us,
            })

        prev = previous_map.get(name)
        if prev is not None:
            prev_us = int(prev["per_iter_us"])
            if prev_us > 0:
                delta_pct = (per_iter - prev_us) / prev_us * 100.0
                record["prev_per_iter_us"] = prev_us
                record["delta_pct"] = round(delta_pct, 2)
                if delta_pct > regression_pct:
                    record["passed_regression"] = False
                    regressions.append({
                        "name": name,
                        "per_iter_us": per_iter,
                        "prev_per_iter_us": prev_us,
                        "delta_pct": round(delta_pct, 2),
                        "limit_pct": regression_pct,
                    })
        checked.append(record)

    for name in current_map:
        if name not in threshold_names:
            unknown.append(name)

    report = {
        "current_run": current.get("timestamp"),
        "current_commit": current.get("git_commit"),
        "previous_run": previous.get("timestamp") if previous else None,
        "regression_pct_limit": regression_pct,
        "policy": {
            "fail_on_missing": fail_on_missing,
            "fail_on_unknown": fail_on_unknown,
            "allow_zero_target": allow_zero_target,
        },
        "checked": checked,
        "missing": missing,
        "violations": violations,
        "regressions": regressions,
        "unknown": unknown,
    }

    print(f"Gate report: {current.get('timestamp', 'unknown')}")
    print(f"  Checked:     {len(checked)}")
    print(f"  Missing:     {len(missing)} {missing if missing else ''}")
    print(f"  Violations:  {len(violations)}")
    for v in violations:
        print(
            f"    [VIOLATION] {v['name']}: "
            f"{v['per_iter_us']}us > max {v['max_us']}us"
        )
    print(f"  Regressions: {len(regressions)} (limit {regression_pct}%)")
    for r in regressions:
        print(
            f"    [REGRESSION] {r['name']}: "
            f"{r['per_iter_us']}us vs {r['prev_per_iter_us']}us "
            f"({r['delta_pct']:+.2f}%)"
        )
    print(f"  Unknown:     {len(unknown)} {unknown if unknown else ''}")

    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json, "w") as f:
            json.dump(report, f, indent=2)
        print(f"  JSON report: {args.json}")

    if missing and fail_on_missing:
        print(f"GATE FAIL: {len(missing)} required benchmark(s) missing")
        return GATE_EXIT_MISSING
    if violations:
        print(f"GATE FAIL: {len(violations)} threshold violation(s)")
        return GATE_EXIT_THRESHOLD
    if regressions:
        print(f"GATE FAIL: {len(regressions)} regression(s) beyond {regression_pct}%")
        return GATE_EXIT_REGRESSION
    if unknown and fail_on_unknown:
        print(f"GATE FAIL: {len(unknown)} unknown benchmark(s) (fail_on_unknown=true)")
        return GATE_EXIT_UNKNOWN

    print("GATE PASS")
    return GATE_EXIT_PASS


def main() -> None:
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    command = sys.argv[1]

    if command == "store":
        lines = sys.stdin.readlines()
        store_results(lines)
    elif command == "compare":
        compare_runs()
    elif command == "history":
        if len(sys.argv) < 3:
            print("Usage: bench_compare.py history <benchmark_name>")
            sys.exit(1)
        show_history(sys.argv[2])
    elif command == "list":
        list_runs()
    elif command == "gate":
        sys.exit(gate(sys.argv[2:]))
    else:
        print(f"Unknown command: {command}")
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
