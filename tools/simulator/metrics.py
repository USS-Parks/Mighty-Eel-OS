from __future__ import annotations

import json
import statistics


class MetricsCollector:
    def __init__(self) -> None:
        self.latencies: list[float] = []
        self.token_rates: list[float] = []
        self.batch_sizes: list[int] = []
        self.queue_depths: list[int] = []
        self.eviction_count = 0
        self.admission_count = 0
        self.request_count = 0
        self.completed_count = 0
        self.thrash_count = 0
        self.kv_utilization_samples: list[float] = []
        self.violations: dict[str, int] = {}

    def record_latency(self, latency_ms: float) -> None:
        self.latencies.append(latency_ms)

    def record_token_rate(self, tokens_per_sec: float) -> None:
        self.token_rates.append(tokens_per_sec)

    def record_batch(self, batch_size: int) -> None:
        self.batch_sizes.append(batch_size)

    def record_queue_depth(self, depth: int) -> None:
        self.queue_depths.append(depth)

    def record_eviction(self) -> None:
        self.eviction_count += 1

    def record_admission(self) -> None:
        self.admission_count += 1

    def record_request(self) -> None:
        self.request_count += 1

    def record_completion(self) -> None:
        self.completed_count += 1

    def record_thrash(self) -> None:
        self.thrash_count += 1

    def record_kv_utilization(self, util: float) -> None:
        self.kv_utilization_samples.append(util)

    def record_violation(self, name: str) -> None:
        self.violations[name] = self.violations.get(name, 0) + 1

    def percentile(self, data: list[float], p: float) -> float:
        if not data:
            return 0.0
        sorted_data = sorted(data)
        k = (len(sorted_data) - 1) * p / 100.0
        f = int(k)
        c = f + 1
        if c >= len(sorted_data):
            return sorted_data[f]
        return sorted_data[f] + (k - f) * (sorted_data[c] - sorted_data[f])

    def report(self) -> dict:
        throughput_tps = statistics.mean(self.token_rates) if self.token_rates else 0.0
        requests_per_sec = self.completed_count / max(sum(self.latencies) / 1000.0, 0.001)
        return {
            "requests_total": self.request_count,
            "completed": self.completed_count,
            "throughput_tokens_per_sec": round(throughput_tps, 2),
            "requests_per_sec": round(requests_per_sec, 2),
            "latency_ms_p50": round(self.percentile(self.latencies, 50), 2),
            "latency_ms_p95": round(self.percentile(self.latencies, 95), 2),
            "latency_ms_p99": round(self.percentile(self.latencies, 99), 2),
            "avg_batch_size": round(statistics.mean(self.batch_sizes), 2) if self.batch_sizes else 0.0,
            "batch_utilization_pct": round(statistics.mean(self.batch_sizes) / max(*(self.batch_sizes or [1]), 1) * 100, 1) if self.batch_sizes else 0.0,
            "evictions": self.eviction_count,
            "admissions": self.admission_count,
            "thrash_events": self.thrash_count,
            "evictions_per_sec": round(self.eviction_count / max(self.completed_count, 1), 2),
            "avg_kv_utilization_pct": round(statistics.mean(self.kv_utilization_samples) * 100, 1) if self.kv_utilization_samples else 0.0,
            "violations": self.violations,
        }

    def report_json(self, indent: int = 2) -> str:
        return json.dumps(self.report(), indent=indent)

    def reset(self) -> None:
        self.__init__()
