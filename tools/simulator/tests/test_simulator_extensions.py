"""Tests for simulator extensions: trace_generator and hybrid."""

from __future__ import annotations

import importlib.util
import json
import random
import sys
from datetime import UTC
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SIM = ROOT / "tools" / "simulator"


def _load(name: str):
    spec = importlib.util.spec_from_file_location(
        f"simulator.{name}", SIM / f"{name}.py"
    )
    assert spec and spec.loader, f"could not load {name}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[f"simulator.{name}"] = module
    spec.loader.exec_module(module)
    return module


trace_generator = _load("trace_generator")
hybrid = _load("hybrid")
TraceGenerator = trace_generator.TraceGenerator
HybridWorkload = hybrid.HybridWorkload
SpikeConfig = hybrid.SpikeConfig


def _write_trace(path: Path, offsets: list[float]) -> None:
    base_secs = 1_700_000_000
    with path.open("w", encoding="utf-8") as dst:
        for i, off in enumerate(offsets):
            ts = _to_iso(base_secs + off)
            event = {
                "timestamp": ts,
                "request_id": f"00000000-0000-0000-0000-{i:012d}",
                "session_id_hash": f"sess-{i % 3}",
                "model_alias": "qwen3-14b",
                "input_tokens": 100 + i,
                "output_tokens": 50,
                "latency_ms": 200,
                "queue_wait_ms": 5,
                "priority": "normal",
                "was_continuation": i % 3 != 0,
            }
            dst.write(json.dumps(event))
            dst.write("\n")


def _to_iso(epoch_secs: float) -> str:
    from datetime import datetime

    return datetime.fromtimestamp(epoch_secs, tz=UTC).isoformat()


def test_trace_generator_preserves_inter_request_gaps(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, offsets=[0.0, 5.0, 7.0, 30.0])
    gen = TraceGenerator(trace)
    rng = random.Random(0)

    # sim_time = 0: only first event is due
    first = gen.generate(0.0, rng)
    assert first["seq_id"].endswith("000000000000")
    assert gen.generate(0.0, rng) is None  # second event is at offset 5

    # sim_time = 6: second event due, third not
    second = gen.generate(6.0, rng)
    assert second["seq_id"].endswith("000000000001")
    assert gen.generate(6.0, rng) is None  # third is at offset 7

    # sim_time = 7: third event due
    third = gen.generate(7.0, rng)
    assert third["seq_id"].endswith("000000000002")

    # sim_time = 30: fourth event due
    fourth = gen.generate(30.0, rng)
    assert fourth["seq_id"].endswith("000000000003")
    assert gen.generate(30.0, rng) is None


def test_trace_generator_marks_continuations(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, offsets=[0.0, 1.0, 2.0])
    gen = TraceGenerator(trace)
    rng = random.Random(0)

    e0 = gen.generate(10.0, rng)
    e1 = gen.generate(10.0, rng)
    e2 = gen.generate(10.0, rng)
    assert e0["continuation_of"] is None
    assert e1["continuation_of"] is not None
    assert e2["continuation_of"] is not None


def test_trace_generator_time_scale_compresses_timeline(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, offsets=[0.0, 10.0])
    gen = TraceGenerator(trace, time_scale=2.0)
    rng = random.Random(0)

    # Original offset 10s becomes 5s after time_scale=2 compression.
    assert gen.generate(0.0, rng) is not None
    assert gen.generate(4.99, rng) is None
    assert gen.generate(5.0, rng) is not None


def test_hybrid_emits_spike_during_window(tmp_path: Path) -> None:
    trace = tmp_path / "trace.ndjson"
    _write_trace(trace, offsets=[0.0])
    baseline = TraceGenerator(trace)
    spike = SpikeConfig(start_time=10.0, duration=5.0, requests_per_sec=2.0)
    hyb = HybridWorkload(baseline, spike)
    rng = random.Random(0)

    # Before the spike window: only baseline.
    pre = hyb.generate(1.0, rng)
    assert pre["seq_id"].endswith("000000000000")
    assert pre.get("spike") is not True

    # In-window first call: spike fires unconditionally.
    inside = hyb.generate(10.0, rng)
    assert inside["model_alias"] == "spike"
    assert inside.get("spike") is True
    assert inside["priority"] == "high"

    # After the spike window closes, no more spike events.
    rng2 = random.Random(7)
    post = HybridWorkload(TraceGenerator(trace), spike)
    post.generate(0.0, rng2)
    after = post.generate(20.0, rng2)
    assert after is None or after.get("spike") is not True


def test_spike_config_validates() -> None:
    import pytest

    with pytest.raises(ValueError):
        SpikeConfig(start_time=0.0, duration=0.0, requests_per_sec=1.0)
    with pytest.raises(ValueError):
        SpikeConfig(start_time=0.0, duration=1.0, requests_per_sec=0.0)
