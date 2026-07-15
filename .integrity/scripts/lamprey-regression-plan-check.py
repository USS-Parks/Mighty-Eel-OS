#!/usr/bin/env python3
"""Validate and optionally execute the Lamprey M0 regression/reachability plan."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
REGISTER = ROOT / "docs/scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json"
PLAN = ROOT / "test-evidence/lamprey-saddle-hardening/M0/regression-plan.json"


def load(path: Path) -> dict:
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def indexed(rows: list[dict], label: str) -> dict[str, dict]:
    result: dict[str, dict] = {}
    for row in rows:
        row_id = row.get("id")
        if not isinstance(row_id, str) or not row_id:
            raise ValueError(f"{label} row lacks a stable id")
        if row_id in result:
            raise ValueError(f"duplicate {label} id {row_id}")
        result[row_id] = row
    return result


def require_text(row: dict, keys: tuple[str, ...], row_id: str) -> None:
    for key in keys:
        value = row.get(key)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"{row_id} lacks required text field {key}")


def validate(run_reachability: bool) -> None:
    register = load(REGISTER)
    plan = load(PLAN)
    registered_confirmed = indexed(register["confirmed"], "registered confirmed")
    registered_deferred = indexed(register["deferred"], "registered deferred")
    planned_confirmed = indexed(plan["confirmed"], "planned confirmed")
    planned_deferred = indexed(plan["deferred"], "planned deferred")

    if set(registered_confirmed) != set(planned_confirmed):
        raise ValueError("confirmed plan IDs do not exactly match the finding register")
    if set(registered_deferred) != set(planned_deferred):
        raise ValueError("deferred plan IDs do not exactly match the finding register")

    allowed_modes = {
        "unit",
        "isolated-integration",
        "deterministic-concurrency",
        "fault-injection",
        "request-fixture",
    }
    regressions: set[str] = set()
    for row_id, row in planned_confirmed.items():
        require_text(
            row,
            ("regression", "boundary", "fixture", "mode", "red", "green"),
            row_id,
        )
        if row["regression"] != registered_confirmed[row_id]["regression"]:
            raise ValueError(f"{row_id} regression ID differs from the finding register")
        if row["regression"] in regressions:
            raise ValueError(f"duplicate regression ID {row['regression']}")
        regressions.add(row["regression"])
        if row["mode"] not in allowed_modes:
            raise ValueError(f"{row_id} has unsupported execution mode {row['mode']}")

    for row_id, row in planned_deferred.items():
        require_text(row, ("query_id", "question", "expected"), row_id)
        command = row.get("command")
        if not isinstance(command, list) or not command or not all(
            isinstance(part, str) and part for part in command
        ):
            raise ValueError(f"{row_id} lacks an argv-form executable command")
        if command[0] != "rg":
            raise ValueError(f"{row_id} reachability command must be read-only rg")
        if run_reachability:
            completed = subprocess.run(
                command,
                cwd=ROOT,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
            )
            if completed.returncode not in (0, 1):
                detail = (completed.stderr or completed.stdout).strip()
                raise ValueError(
                    f"{row_id} reachability command failed ({completed.returncode}): {detail}"
                )
            outcome = "matches" if completed.returncode == 0 else "no matches"
            print(f"{row_id}: {outcome}")

    print(
        "OK — "
        f"{len(planned_confirmed)} confirmed red-to-green plans and "
        f"{len(planned_deferred)} executable deferred reachability questions"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--run-reachability",
        action="store_true",
        help="execute the read-only deferred rg questions",
    )
    args = parser.parse_args()
    try:
        validate(args.run_reachability)
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
        print(f"ERROR — {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
