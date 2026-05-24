"""Reconstruct sessions from a trace by grouping events on session_id_hash.

The simulator needs per-session metrics (inter-request gaps, lifetimes, token
totals) for calibration of KV reuse coefficients and for reuse-probability
testing in replay mode. This tool turns an event-level NDJSON trace into a
session-level NDJSON file with one record per session.

Usage:
    python reconstruct.py <trace.ndjson> <sessions.ndjson>
"""

from __future__ import annotations

import argparse
import itertools
import json
import statistics
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path


def parse_timestamp(value: str) -> float:
    """Parse an RFC 3339 timestamp into seconds since the unix epoch."""
    if value.endswith("Z"):
        value = value[:-1] + "+00:00"
    return datetime.fromisoformat(value).timestamp()


def reconstruct(events: list[dict]) -> list[dict]:
    """Group events by session and compute per-session statistics."""
    by_session: dict[str, list[dict]] = defaultdict(list)
    for event in events:
        session = event.get("session_id_hash")
        if session is None:
            continue
        by_session[session].append(event)

    sessions: list[dict] = []
    for session_id, items in by_session.items():
        items.sort(key=lambda ev: ev.get("timestamp", ""))
        times = [parse_timestamp(ev["timestamp"]) for ev in items]
        gaps_secs = [b - a for a, b in itertools.pairwise(times)] if len(times) > 1 else []
        total_input = sum(int(ev.get("input_tokens", 0)) for ev in items)
        total_output = sum(int(ev.get("output_tokens", 0)) for ev in items)
        first_seen = items[0].get("timestamp")
        last_seen = items[-1].get("timestamp")
        duration_secs = times[-1] - times[0] if len(times) > 1 else 0.0
        record = {
            "session_id_hash": session_id,
            "first_seen": first_seen,
            "last_seen": last_seen,
            "request_count": len(items),
            "duration_secs": round(duration_secs, 6),
            "total_input_tokens": total_input,
            "total_output_tokens": total_output,
            "mean_gap_secs": round(statistics.fmean(gaps_secs), 6) if gaps_secs else 0.0,
            "max_gap_secs": round(max(gaps_secs), 6) if gaps_secs else 0.0,
            "min_gap_secs": round(min(gaps_secs), 6) if gaps_secs else 0.0,
            "model_aliases": sorted({ev.get("model_alias", "") for ev in items}),
            "had_continuation": any(ev.get("was_continuation") for ev in items),
        }
        sessions.append(record)
    sessions.sort(key=lambda s: s["first_seen"] or "")
    return sessions


def process(input_path: Path, output_path: Path) -> int:
    events: list[dict] = []
    with input_path.open("r", encoding="utf-8") as src:
        for line_no, line in enumerate(src, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError as exc:
                raise ValueError(f"line {line_no}: invalid JSON: {exc}") from exc

    sessions = reconstruct(events)
    with output_path.open("w", encoding="utf-8") as dst:
        for record in sessions:
            dst.write(json.dumps(record, sort_keys=True))
            dst.write("\n")
    return len(sessions)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Group MAI trace events into per-session records."
    )
    parser.add_argument("input", type=Path, help="Input trace NDJSON file.")
    parser.add_argument("output", type=Path, help="Output session NDJSON path.")
    args = parser.parse_args(argv)

    if not args.input.exists():
        print(f"error: input {args.input} does not exist", file=sys.stderr)
        return 2

    count = process(args.input, args.output)
    print(f"reconstructed {count} sessions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
