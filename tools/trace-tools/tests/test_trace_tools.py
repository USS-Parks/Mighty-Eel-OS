"""Tests for trace tooling.

Covers:
- anonymize.py: schema enforcement, salt-driven re-hashing, privacy assertions.
- reconstruct.py: session grouping, gap statistics, ordering.
- calibrate.py: stable output on empty input, plausible coefficients on real data.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TRACE_TOOLS = ROOT / "tools" / "trace-tools"


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, TRACE_TOOLS / f"{name}.py")
    assert spec and spec.loader, f"could not load {name}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


anonymize = _load("anonymize")
reconstruct = _load("reconstruct")
calibrate = _load("calibrate")


def _event(
    *,
    ts: str,
    session_hash: str = "abc",
    request_id: str = "00000000-0000-0000-0000-000000000001",
    input_tokens: int = 128,
    output_tokens: int = 256,
    latency_ms: int = 200,
    queue_wait_ms: int = 10,
    priority: str = "normal",
    was_continuation: bool = False,
    extra: dict | None = None,
) -> dict:
    base = {
        "timestamp": ts,
        "request_id": request_id,
        "session_id_hash": session_hash,
        "model_alias": "qwen3-14b",
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "latency_ms": latency_ms,
        "queue_wait_ms": queue_wait_ms,
        "priority": priority,
        "was_continuation": was_continuation,
    }
    if extra:
        base.update(extra)
    return base


def _write_ndjson(path: Path, records: list[dict]) -> None:
    with path.open("w", encoding="utf-8") as dst:
        for record in records:
            dst.write(json.dumps(record))
            dst.write("\n")


def test_anonymize_strips_disallowed_fields_and_rehashes(tmp_path: Path) -> None:
    src = tmp_path / "raw.ndjson"
    dst = tmp_path / "anon.ndjson"
    _write_ndjson(
        src,
        [
            _event(
                ts="2026-05-22T12:00:00+00:00",
                session_hash="original-hash",
                extra={"prompt_text": "leaked", "user_id": "should-be-stripped"},
            )
        ],
    )

    count = anonymize.process(src, dst, salt="run-salt")
    assert count == 1

    lines = dst.read_text(encoding="utf-8").strip().splitlines()
    event = json.loads(lines[0])

    # No disallowed fields survive.
    assert "prompt_text" not in event
    assert "user_id" not in event
    # Session id was re-hashed (not equal to original).
    assert event["session_id_hash"] != "original-hash"
    assert len(event["session_id_hash"]) == 32

    # Re-running with same salt produces same hash.
    again = tmp_path / "anon2.ndjson"
    anonymize.process(src, again, salt="run-salt")
    rehashed = json.loads(again.read_text(encoding="utf-8").strip().splitlines()[0])
    assert rehashed["session_id_hash"] == event["session_id_hash"]


def test_anonymize_validate_rejects_extra_fields() -> None:
    import pytest

    bad = {"timestamp": "x", "request_id": "y", "session_id_hash": "z", "leaked": True}
    with pytest.raises(ValueError):
        anonymize.validate(bad)


def test_reconstruct_groups_events_by_session_and_computes_gaps() -> None:
    events = [
        _event(ts="2026-05-22T12:00:00+00:00", session_hash="A", request_id="r1"),
        _event(
            ts="2026-05-22T12:00:05+00:00",
            session_hash="A",
            request_id="r2",
            was_continuation=True,
        ),
        _event(ts="2026-05-22T12:00:10+00:00", session_hash="A", request_id="r3"),
        _event(ts="2026-05-22T12:00:01+00:00", session_hash="B", request_id="r4"),
    ]
    sessions = reconstruct.reconstruct(events)
    assert len(sessions) == 2
    by_id = {s["session_id_hash"]: s for s in sessions}
    a = by_id["A"]
    b = by_id["B"]

    assert a["request_count"] == 3
    assert a["mean_gap_secs"] == 5.0
    assert a["max_gap_secs"] == 5.0
    assert a["had_continuation"] is True
    assert b["request_count"] == 1
    assert b["had_continuation"] is False


def test_calibrate_returns_defaults_for_empty_input() -> None:
    result = calibrate.calibrate([])
    assert result["sessions_observed"] == 0
    assert result["reuse_alpha"] == 0.1
    assert result["reuse_beta"] == 0.1


def test_calibrate_increases_alpha_with_repeat_traffic() -> None:
    repeat_heavy = [
        {"request_count": 4, "mean_gap_secs": 2.0} for _ in range(8)
    ] + [{"request_count": 1, "mean_gap_secs": 0.0} for _ in range(2)]
    one_shot_heavy = [{"request_count": 1, "mean_gap_secs": 0.0} for _ in range(10)]

    heavy = calibrate.calibrate(repeat_heavy)
    light = calibrate.calibrate(one_shot_heavy)
    assert heavy["reuse_alpha"] > light["reuse_alpha"]
    assert 0.1 <= heavy["reuse_alpha"] <= 1.0
    assert heavy["reuse_beta"] > 0.1


def test_calibrate_render_toml_includes_coefficients() -> None:
    result = calibrate.calibrate(
        [{"request_count": 3, "mean_gap_secs": 5.0} for _ in range(5)]
    )
    rendered = calibrate.render_toml(result)
    assert "[eviction]" in rendered
    assert "reuse_alpha" in rendered
    assert "reuse_beta" in rendered
