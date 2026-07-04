"""Trace-driven policy comparison harness.

Replays an NDJSON trace through multiple KV policies and emits a comparison
report. Deterministic given the same seed + trace + policy. The output is
suitable for the report generator (`report.py`) and for direct inclusion in
acquisition documentation.

Usage:
    python replay_compare.py <trace.ndjson> [--policy lru] [--policy heuristic]
                              [--seed 42] [--sim-time SECS] [--vram-gb 0.1]
                              [--out comparison.json]
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from pathlib import Path
from typing import Any

# Allow running as a script from tools/simulator/ without packaging it.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from engine import SimEngine, SimulationEvent
from gpu import GpuModel
from kv_policy import (
    BatchAwareKvManager,
    HeuristicScoredKvManager,
    LruKvManager,
    SizeBasedKvManager,
)
from metrics import MetricsCollector
from trace_generator import TraceGenerator

KV_POLICIES = {
    "lru": LruKvManager,
    "size": SizeBasedKvManager,
    "heuristic": HeuristicScoredKvManager,
    "batch_aware": BatchAwareKvManager,
}


def run_trace_replay(
    trace_path: Path,
    kv_policy: str,
    seed: int = 42,
    sim_time: float | None = None,
    vram_gb: float = 0.1,
    time_scale: float = 1.0,
) -> dict[str, Any]:
    """Run one trace through one policy. Returns a metrics report dict."""
    if kv_policy not in KV_POLICIES:
        raise ValueError(
            f"unknown kv policy '{kv_policy}'; "
            f"choices: {sorted(KV_POLICIES)}"
        )

    rng = random.Random(seed)
    engine = SimEngine(seed=seed)
    gpu = GpuModel(
        total_vram=vram_gb * 1e9, base_step_time=0.050, scaling_factor=0.005
    )
    engine.gpu_model = gpu
    kv_manager = KV_POLICIES[kv_policy](
        total_bytes=gpu.total_vram,
        min_residency_time=0.0,
        recently_evicted_penalty=5.0,
    )
    engine.kv_manager = kv_manager

    trace_gen = TraceGenerator(trace_path, time_scale=time_scale)
    if sim_time is None:
        offsets = trace_gen._offsets
        sim_time = max(offsets) + 1.0 if offsets else 1.0

    metrics = MetricsCollector()
    engine.instances = [
        {"id": "gpu-0", "model": "default", "queue_depth": 0, "max_batch": 64}
    ]

    class TraceRequestGenerator:
        def generate(self, sim_time: float) -> dict:
            event = trace_gen.generate(sim_time, rng)
            if event is None:
                return {
                    "type": "none",
                    "prompt_tokens": 0,
                    "max_tokens": 0,
                    "estimated_kv_bytes": 0,
                }
            return event

    engine.request_generator = TraceRequestGenerator()

    class ReplayScheduler:
        def select_instance(self, req: dict, instances: list[dict]) -> str | None:
            if req.get("type") == "none":
                return None
            metrics.record_request()
            return instances[0]["id"]

    engine.scheduler = ReplayScheduler()

    tick = 0.1
    ticks = int(sim_time / tick) + 1
    for i in range(ticks):
        engine.schedule_event(SimulationEvent(i * tick, "request_arrive"))

    engine.run(max_time=sim_time)

    for record in engine.results:
        if record["kind"] == "completed":
            metrics.record_completion()
            metrics.record_latency(record["latency_ms"])
            metrics.record_token_rate(record.get("token_rate", 0))
        elif record["kind"] == "eviction":
            metrics.record_eviction()
        elif record["kind"] == "admission":
            metrics.record_admission()
        if kv_manager.total_bytes > 0:
            metrics.record_kv_utilization(
                kv_manager.used_bytes / kv_manager.total_bytes
            )

    report = metrics.report()
    report["policy"] = kv_policy
    report["seed"] = seed
    report["sim_time_secs"] = round(sim_time, 4)
    report["trace_events"] = trace_gen.total
    report["vram_gb"] = vram_gb
    return report


def compare_policies_on_trace(
    trace_path: Path,
    policies: list[str] | None = None,
    seed: int = 42,
    sim_time: float | None = None,
    vram_gb: float = 0.1,
) -> dict[str, Any]:
    """Run a trace through each policy and aggregate the reports."""
    chosen = policies or list(KV_POLICIES.keys())
    results: dict[str, Any] = {}
    for policy in chosen:
        results[policy] = run_trace_replay(
            trace_path,
            kv_policy=policy,
            seed=seed,
            sim_time=sim_time,
            vram_gb=vram_gb,
        )
    return {
        "trace_path": str(trace_path),
        "seed": seed,
        "vram_gb": vram_gb,
        "sim_time_secs": results[chosen[0]]["sim_time_secs"] if results else 0.0,
        "policies": results,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run an NDJSON trace through one or more KV policies."
    )
    parser.add_argument("trace", type=Path, help="Input trace NDJSON file.")
    parser.add_argument(
        "--policy",
        action="append",
        default=None,
        help="KV policy to compare (repeatable). Default: all four.",
    )
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--sim-time", type=float, default=None)
    parser.add_argument("--vram-gb", type=float, default=0.1)
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="JSON output path (stdout if omitted).",
    )
    args = parser.parse_args(argv)

    if not args.trace.exists():
        print(f"error: trace {args.trace} does not exist", file=sys.stderr)
        return 2

    comparison = compare_policies_on_trace(
        args.trace,
        policies=args.policy,
        seed=args.seed,
        sim_time=args.sim_time,
        vram_gb=args.vram_gb,
    )
    payload = json.dumps(comparison, indent=2)
    if args.out:
        args.out.write_text(payload, encoding="utf-8")
        print(f"wrote comparison to {args.out}")
    else:
        sys.stdout.write(payload)
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
