from __future__ import annotations

import json
import random
import sys
import time
from pathlib import Path
from typing import Any

from engine import SimEngine, SimulationEvent
from gpu import GpuModel
from kv_policy import (
    BatchAwareKvManager,
    HeuristicScoredKvManager,
    LruKvManager,
    SizeBasedKvManager,
)
from metrics import MetricsCollector
from workload import MixedWorkload


def run_experiment(
    kv_policy_name: str = "heuristic",
    workload_config: dict | None = None,
    gpu_config: dict | None = None,
    sim_time: float = 60.0,
    seed: int = 42,
    vram_gb: float = 0.1,
) -> MetricsCollector:
    rng = random.Random(seed)
    engine = SimEngine(seed=seed)
    gpu_config = gpu_config or {}
    gpu_config["total_vram"] = vram_gb * 1e9  # vram_gb is for clarity; < 2 works for pressure
    gpu_config["base_step_time"] = 0.050
    gpu_config["scaling_factor"] = 0.005
    gpu = GpuModel(**(gpu_config))
    engine.gpu_model = gpu

    kv_class = {
        "lru": LruKvManager,
        "size": SizeBasedKvManager,
        "heuristic": HeuristicScoredKvManager,
        "batch_aware": BatchAwareKvManager,
    }.get(kv_policy_name, HeuristicScoredKvManager)
    kv_manager = kv_class(total_bytes=gpu.total_vram, min_residency_time=0.0, recently_evicted_penalty=5.0)
    engine.kv_manager = kv_manager

    wc = workload_config or {}
    chat_config = {
        "arrival_rate": wc.get("arrival_rate", 20.0),
        "session_length_mean": wc.get("session_length_mean", 3),
        "session_length_std": wc.get("session_length_std", 1),
        "prompt_tokens_mean": wc.get("chat_prompt_tokens_mean", 3000),
        "prompt_tokens_std": wc.get("chat_prompt_tokens_std", 1500),
        "max_tokens_mean": wc.get("chat_max_tokens_mean", 2000),
        "max_tokens_std": wc.get("chat_max_tokens_std", 1000),
        "reuse_rate": wc.get("chat_reuse_rate", 0.3),
    }
    batch_config = {
        "arrival_rate": wc.get("batch_arrival_rate", 5.0),
        "prompt_tokens_mean": wc.get("batch_prompt_tokens_mean", 8000),
        "prompt_tokens_std": wc.get("batch_prompt_tokens_std", 4000),
        "max_tokens_mean": wc.get("batch_max_tokens_mean", 2000),
        "max_tokens_std": wc.get("batch_max_tokens_std", 1000),
    }
    workload = MixedWorkload(
        chat_ratio=wc.get("chat_ratio", 0.5),
        arrival_rate=wc.get("arrival_rate", 20.0),
        chat_config=chat_config,
        batch_config=batch_config,
    )
    metrics = MetricsCollector()

    engine.instances = [
        {"id": "gpu-0", "model": "default", "queue_depth": 0, "max_batch": 64}
    ]

    class RequestGenerator:
        def generate(self, sim_time: float) -> dict:
            return workload.generate(sim_time, rng) or {
                "type": "none", "prompt_tokens": 0, "max_tokens": 0, "estimated_kv_bytes": 0
            }

    engine.request_generator = RequestGenerator()

    class SchedulerStub:
        def select_instance(self, req: dict, instances: list[dict]) -> str | None:
            if req.get("type") == "none":
                return None
            metrics.record_request()
            return instances[0]["id"]

    engine.scheduler = SchedulerStub()

    for i in range(int(sim_time / 0.1)):
        engine.schedule_event(SimulationEvent(i * 0.1, "request_arrive"))

    engine.run(max_time=sim_time)

    for r in engine.results:
        if r["kind"] == "completed":
            metrics.record_completion()
            metrics.record_latency(r["latency_ms"])
            metrics.record_token_rate(r.get("token_rate", 0))
        elif r["kind"] == "eviction":
            metrics.record_eviction()
        elif r["kind"] == "admission":
            metrics.record_admission()
        if kv_manager.total_bytes > 0:
            metrics.record_kv_utilization(kv_manager.used_bytes / kv_manager.total_bytes)

    return metrics


def compare_policies(sim_time: float = 30.0, seed: int = 42, vram_gb: float = 0.1) -> dict[str, Any]:
    policies = ["lru", "size", "heuristic", "batch_aware"]
    results: dict[str, Any] = {}
    for policy in policies:
        m = run_experiment(kv_policy_name=policy, sim_time=sim_time, seed=seed, vram_gb=vram_gb)
        results[policy] = m.report()
    return results


def memory_pressure_sweep(sim_time: float = 30.0, seed: int = 42) -> list[dict[str, Any]]:
    vram_sizes = [0.05, 0.1, 0.2, 0.5, 1.0]
    results: list[dict[str, Any]] = []
    for vram in vram_sizes:
        m = run_experiment(kv_policy_name="heuristic", sim_time=sim_time, seed=seed, vram_gb=vram)
        r = m.report()
        r["vram_gb"] = vram
        results.append(r)
    return results


def workload_mix_sweep(step: float = 0.25, sim_time: float = 30.0, seed: int = 42) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    chat_ratio = 0.0
    while chat_ratio <= 1.0:
        wc = {"chat_ratio": round(chat_ratio, 2), "arrival_rate": 20.0}
        m = run_experiment(kv_policy_name="heuristic", workload_config=wc, sim_time=sim_time, seed=seed)
        r = m.report()
        r["chat_ratio"] = round(chat_ratio, 2)
        results.append(r)
        chat_ratio += step
    return results


def burst_load_test(sim_time: float = 60.0, seed: int = 42, vram_gb: float = 0.1) -> dict[str, Any]:
    rng = random.Random(seed)
    engine = SimEngine(seed=seed)
    gpu = GpuModel(total_vram=vram_gb * 1e9, base_step_time=0.050, scaling_factor=0.005)
    engine.gpu_model = gpu
    kv = HeuristicScoredKvManager(total_bytes=gpu.total_vram, min_residency_time=0.0, recently_evicted_penalty=5.0)
    engine.kv_manager = kv
    metrics = MetricsCollector()

    engine.instances = [{"id": "gpu-0", "model": "default", "queue_depth": 0, "max_batch": 64}]
    burst_active = [False]

    class BurstRequestGenerator:
        def generate(self, sim_time: float) -> dict:
            is_burst = 10 <= sim_time <= 20
            if is_burst != burst_active[0]:
                burst_active[0] = is_burst
            rate = 15.0 if is_burst else 3.0
            if rng.random() > rate / 10.0:
                return {"type": "none", "prompt_tokens": 0, "max_tokens": 0, "estimated_kv_bytes": 0}
            prompt_tokens = max(50, int(rng.gauss(1000, 500)))
            max_tokens = max(50, int(rng.gauss(500, 250)))
            estimated_kv_bytes = (prompt_tokens + max_tokens) * 2.0 * 1024
            return {
                "type": "chat", "session_id": f"burst_{sim_time}",
                "prompt_tokens": prompt_tokens,
                "max_tokens": max_tokens,
                "estimated_kv_bytes": estimated_kv_bytes,
                "seq_id": f"seq_{sim_time}_{rng.randint(0, 99999)}",
                "continuation_of": None,
            }

    engine.request_generator = BurstRequestGenerator()

    class BurstScheduler:
        def select_instance(self, req: dict, instances: list[dict]) -> str | None:
            if req.get("type") == "none":
                return None
            metrics.record_request()
            return instances[0]["id"]

    engine.scheduler = BurstScheduler()

    for i in range(int(sim_time / 0.1)):
        engine.schedule_event(SimulationEvent(i * 0.1, "request_arrive"))
    engine.run(max_time=sim_time)

    for r in engine.results:
        if r["kind"] == "completed":
            metrics.record_completion()
            metrics.record_latency(r["latency_ms"])
            metrics.record_token_rate(r.get("token_rate", 0))
        elif r["kind"] == "eviction":
            metrics.record_eviction()
        if kv.total_bytes > 0:
            metrics.record_kv_utilization(kv.used_bytes / kv.total_bytes)

    return metrics.report()


def weight_sensitivity(sim_time: float = 30.0, seed: int = 42, vram_gb: float = 0.1) -> list[dict[str, Any]]:
    weight_configs = [
        {"weight_idle": 0.6, "weight_size": 0.2, "weight_priority": 0.2},
        {"weight_idle": 0.2, "weight_size": 0.6, "weight_priority": 0.2},
        {"weight_idle": 0.2, "weight_size": 0.2, "weight_priority": 0.6},
        {"weight_idle": 0.33, "weight_size": 0.33, "weight_priority": 0.34},
    ]
    results: list[dict[str, Any]] = []
    for weights in weight_configs:
        m = run_experiment(kv_policy_name="heuristic", sim_time=sim_time, seed=seed, vram_gb=vram_gb, workload_config={"arrival_rate": 20.0, "chat_ratio": 0.5})
        r = m.report()
        r["weights"] = weights
        results.append(r)
    return results


def run_all_experiments(output_dir: str | Path = "results", seed: int = 42) -> dict[str, Any]:
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)
    results: dict[str, Any] = {}

    print("=== Policy Comparison ===")
    t0 = time.time()
    results["policy_comparison"] = compare_policies(seed=seed)
    print(f"  Done in {time.time() - t0:.2f}s")
    with open(output_path / "policy_comparison.json", "w") as f:
        json.dump(results["policy_comparison"], f, indent=2)
    for policy, report in results["policy_comparison"].items():
        print(f"  {policy}: throughput={report.get('throughput_tokens_per_sec')} req/s, "
              f"p95_latency={report.get('latency_ms_p95')}ms, "
              f"evictions={report.get('evictions')}, "
              f"evict/s={report.get('evictions_per_sec')}, "
              f"KV={report.get('avg_kv_utilization_pct')}%")

    print("=== Memory Pressure Sweep ===")
    t0 = time.time()
    results["memory_sweep"] = memory_pressure_sweep(seed=seed)
    print(f"  Done in {time.time() - t0:.2f}s")
    with open(output_path / "memory_sweep.json", "w") as f:
        json.dump(results["memory_sweep"], f, indent=2)
    for r in results["memory_sweep"]:
        print(f"  {r.get('vram_gb')}GB: throughput={r.get('throughput_tokens_per_sec')} req/s, "
              f"evictions={r.get('evictions')}, KV={r.get('avg_kv_utilization_pct')}%")

    print("=== Workload Mix Sweep ===")
    t0 = time.time()
    results["workload_mix"] = workload_mix_sweep(seed=seed)
    print(f"  Done in {time.time() - t0:.2f}s")
    with open(output_path / "workload_mix.json", "w") as f:
        json.dump(results["workload_mix"], f, indent=2)
    for r in results["workload_mix"]:
        print(f"  chat_ratio={r.get('chat_ratio')}: throughput={r.get('throughput_tokens_per_sec')} req/s, "
              f"evictions={r.get('evictions')}")

    print("=== Burst Load Test ===")
    t0 = time.time()
    results["burst_load"] = burst_load_test(seed=seed)
    print(f"  Done in {time.time() - t0:.2f}s")
    with open(output_path / "burst_load.json", "w") as f:
        json.dump(results["burst_load"], f, indent=2)
    print(f"  Burst: throughput={results['burst_load'].get('throughput_tokens_per_sec')} req/s, "
          f"p95_latency={results['burst_load'].get('latency_ms_p95')}ms, "
          f"evictions={results['burst_load'].get('evictions')}")

    print("=== Weight Sensitivity ===")
    t0 = time.time()
    results["weight_sensitivity"] = weight_sensitivity(seed=seed)
    print(f"  Done in {time.time() - t0:.2f}s")
    with open(output_path / "weight_sensitivity.json", "w") as f:
        json.dump(results["weight_sensitivity"], f, indent=2)
    for r in results["weight_sensitivity"]:
        print(f"  {r.get('weights')}: evictions={r.get('evictions')}, thrash={r.get('thrash_events')}, KV={r.get('avg_kv_utilization_pct')}%")

    print(f"\nAll results written to {output_path.resolve()}")
    return results


if __name__ == "__main__":
    seed = int(sys.argv[1]) if len(sys.argv) > 1 else 42
    run_all_experiments(seed=seed)
