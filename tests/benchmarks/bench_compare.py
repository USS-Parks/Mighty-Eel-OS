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

Session 10 deliverable.
"""

import json
import os
import re
import sys
from datetime import datetime, timezone
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

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
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
    else:
        print(f"Unknown command: {command}")
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
