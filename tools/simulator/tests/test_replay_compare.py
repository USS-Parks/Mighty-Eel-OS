"""Tests for the Gate C backfill: replay comparison + report."""

from __future__ import annotations

import importlib.util
import json
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SIM = ROOT / "tools" / "simulator"

# Ensure the simulator directory is importable so replay_compare can resolve
# its sibling modules (`engine`, `gpu`, `kv_policy`, ...).
if str(SIM) not in sys.path:
    sys.path.insert(0, str(SIM))


def _load(name: str):
    spec = importlib.util.spec_from_file_location(
        f"_session32_{name}", SIM / f"{name}.py"
    )
    assert spec and spec.loader, f"could not load {name}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[f"_session32_{name}"] = module
    spec.loader.exec_module(module)
    return module


replay_compare = _load("replay_compare")
report = _load("report")


def _iso(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, tz=UTC).isoformat()


def _write_trace(path: Path, n_events: int = 20) -> None:
    base = 1_700_000_000.0
    with path.open("w", encoding="utf-8") as dst:
        for i in range(n_events):
            event = {
                "timestamp": _iso(base + i * 0.5),
                "request_id": f"00000000-0000-0000-0000-{i:012d}",
                "session_id_hash": f"sess-{i % 4}",
                "model_alias": "qwen3-14b",
                "input_tokens": 100,
                "output_tokens": 50,
                "latency_ms": 200,
                "queue_wait_ms": 5,
                "priority": "normal",
                "was_continuation": i >= 4,
            }
            dst.write(json.dumps(event))
            dst.write("\n")


def test_run_trace_replay_emits_required_fields(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, n_events=10)
    out = replay_compare.run_trace_replay(
        trace, kv_policy="heuristic", seed=7, vram_gb=0.1
    )
    for field in (
        "policy",
        "seed",
        "sim_time_secs",
        "trace_events",
        "vram_gb",
        "requests_total",
        "completed",
        "latency_ms_p95",
        "evictions",
        "avg_kv_utilization_pct",
    ):
        assert field in out, f"missing {field} in replay report"
    assert out["policy"] == "heuristic"
    assert out["seed"] == 7
    assert out["trace_events"] == 10


def test_run_trace_replay_is_deterministic(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, n_events=15)
    first = replay_compare.run_trace_replay(
        trace, kv_policy="lru", seed=11, vram_gb=0.1
    )
    second = replay_compare.run_trace_replay(
        trace, kv_policy="lru", seed=11, vram_gb=0.1
    )
    # Headline metrics must match exactly given the same inputs.
    for key in (
        "requests_total",
        "completed",
        "latency_ms_p50",
        "latency_ms_p95",
        "evictions",
    ):
        assert first[key] == second[key], f"{key} not deterministic"


def test_run_trace_replay_rejects_unknown_policy(tmp_path: Path) -> None:
    import pytest

    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, n_events=2)
    with pytest.raises(ValueError):
        replay_compare.run_trace_replay(trace, kv_policy="bogus")


def test_compare_policies_runs_all_when_unspecified(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, n_events=8)
    comparison = replay_compare.compare_policies_on_trace(trace, seed=3)
    assert set(comparison["policies"]) == set(replay_compare.KV_POLICIES)
    assert comparison["trace_path"] == str(trace)


def test_report_markdown_includes_all_policies(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, n_events=6)
    comparison = replay_compare.compare_policies_on_trace(trace, seed=5)
    md = report.render_markdown(comparison)
    assert "MAI Scheduler Trace Replay Comparison" in md
    for policy in replay_compare.KV_POLICIES:
        assert policy in md, f"policy {policy} missing from Markdown report"
    assert "Headline Findings" in md


def test_report_markdown_handles_empty_comparison() -> None:
    rendered = report.render_markdown(
        {"trace_path": "x", "seed": 0, "vram_gb": 0.1, "policies": {}}
    )
    assert "No policy results" in rendered


def test_report_find_best_picks_max_throughput_and_min_p95() -> None:
    comparison = {
        "policies": {
            "alpha": {
                "throughput_tokens_per_sec": 100,
                "latency_ms_p95": 500,
                "evictions": 10,
            },
            "beta": {
                "throughput_tokens_per_sec": 50,
                "latency_ms_p95": 100,
                "evictions": 5,
            },
        }
    }
    best = report.find_best(comparison["policies"])
    assert best["throughput"] == "alpha"
    assert best["p95"] == "beta"
    assert best["evictions"] == "beta"
