"""Trace-driven workload generator for the MAI simulator.

Reads an anonymized NDJSON trace and replays its events as simulator workload.
Inter-request gaps and bursts are preserved exactly; the timeline may be
compressed or expanded by a single `time_scale` factor (1.0 = real-time).

The generator implements the `WorkloadGenerator` protocol declared in
`tools/simulator/workload.py`, so the existing `SimEngine` can consume it
without modification. The dict shape returned by `generate()` matches the
fields the engine and scheduler already expect.
"""

from __future__ import annotations

import json
import random
from datetime import datetime
from pathlib import Path

DEFAULT_BYTES_PER_TOKEN = 2.0 * 1024


def _parse_timestamp(value: str) -> float:
    if value.endswith("Z"):
        value = value[:-1] + "+00:00"
    return datetime.fromisoformat(value).timestamp()


def load_trace(path: Path) -> list[dict]:
    """Load and sort a trace file by timestamp."""
    events: list[dict] = []
    with path.open("r", encoding="utf-8") as src:
        for line_no, line in enumerate(src, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError as exc:
                raise ValueError(f"line {line_no}: invalid JSON: {exc}") from exc
    events.sort(key=lambda ev: _parse_timestamp(ev["timestamp"]))
    return events


class TraceGenerator:
    """Replay a trace as a simulator workload.

    Parameters
    ----------
    trace_path : Path
        NDJSON trace produced by `capture.rs` and (typically) `anonymize.py`.
    time_scale : float
        Multiplier on the relative timeline. `1.0` replays at real speed, `2.0`
        replays in half real time, `0.5` slows the replay to twice real time.
    bytes_per_token : float
        Used to estimate KV cache footprint from the trace's token counts.
    """

    def __init__(
        self,
        trace_path: Path,
        time_scale: float = 1.0,
        bytes_per_token: float = DEFAULT_BYTES_PER_TOKEN,
    ) -> None:
        if time_scale <= 0:
            raise ValueError("time_scale must be > 0")
        self.bytes_per_token = bytes_per_token
        self._events = load_trace(trace_path)
        self._offsets = self._compute_offsets(time_scale)
        self._cursor = 0

    def _compute_offsets(self, time_scale: float) -> list[float]:
        if not self._events:
            return []
        base = _parse_timestamp(self._events[0]["timestamp"])
        return [
            (_parse_timestamp(ev["timestamp"]) - base) / time_scale
            for ev in self._events
        ]

    @property
    def remaining(self) -> int:
        return len(self._events) - self._cursor

    @property
    def total(self) -> int:
        return len(self._events)

    def reset(self) -> None:
        self._cursor = 0

    def generate(self, sim_time: float, rng: random.Random) -> dict | None:
        """Return the next event whose offset has been reached, or None."""
        if self._cursor >= len(self._events):
            return None
        if self._offsets[self._cursor] > sim_time:
            return None
        event = self._events[self._cursor]
        self._cursor += 1
        return self._materialize(event)

    def _materialize(self, event: dict) -> dict:
        prompt_tokens = int(event.get("input_tokens", 0))
        output_tokens = int(event.get("output_tokens", 0))
        max_tokens = max(1, output_tokens)
        session_id = str(event.get("session_id_hash", event.get("request_id", "unknown")))
        return {
            "type": "chat",
            "session_id": session_id,
            "prompt_tokens": prompt_tokens,
            "max_tokens": max_tokens,
            "estimated_kv_bytes": (prompt_tokens + max_tokens) * self.bytes_per_token,
            "seq_id": str(event.get("request_id", session_id)),
            "continuation_of": session_id if event.get("was_continuation") else None,
            "model_alias": event.get("model_alias", ""),
            "priority": event.get("priority", "normal"),
            "trace_latency_ms": int(event.get("latency_ms", 0)),
        }
