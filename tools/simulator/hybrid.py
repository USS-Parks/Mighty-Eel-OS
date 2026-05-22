"""Hybrid workload combining a trace baseline with a synthetic spike.

A real production trace is the most realistic stress for the scheduler, but
capacity planning often needs to see how the system would behave if traffic
spiked above anything ever observed. `HybridWorkload` accepts a baseline
generator (typically `TraceGenerator`) and a synthetic burst configuration; the
burst is emitted on top of the baseline during a configurable window.
"""

from __future__ import annotations

import random
from dataclasses import dataclass

from .trace_generator import DEFAULT_BYTES_PER_TOKEN


@dataclass(frozen=True)
class SpikeConfig:
    """Configuration for the synthetic spike injected on top of a baseline."""

    start_time: float
    duration: float
    requests_per_sec: float
    prompt_tokens_mean: float = 200.0
    max_tokens_mean: float = 100.0
    session_prefix: str = "spike"

    def __post_init__(self) -> None:
        if self.duration <= 0:
            raise ValueError("duration must be > 0")
        if self.requests_per_sec <= 0:
            raise ValueError("requests_per_sec must be > 0")

    @property
    def end_time(self) -> float:
        return self.start_time + self.duration


class HybridWorkload:
    """Combine a baseline generator with a synthetic spike."""

    def __init__(
        self,
        baseline,
        spike: SpikeConfig,
        bytes_per_token: float = DEFAULT_BYTES_PER_TOKEN,
    ) -> None:
        self.baseline = baseline
        self.spike = spike
        self.bytes_per_token = bytes_per_token
        self._spike_sequence = 0
        self._last_emitted_time: float | None = None

    def _spike_due(self, sim_time: float, rng: random.Random) -> bool:
        if sim_time < self.spike.start_time or sim_time >= self.spike.end_time:
            return False
        if self._last_emitted_time is None:
            return True
        gap = 1.0 / self.spike.requests_per_sec
        elapsed = sim_time - self._last_emitted_time
        if elapsed >= gap:
            return True
        # Probabilistic emission so we do not over- or under-shoot the target
        # rate when the engine ticks at irregular intervals.
        return rng.random() < (elapsed / gap)

    def _emit_spike(self, sim_time: float) -> dict:
        self._spike_sequence += 1
        self._last_emitted_time = sim_time
        session_id = f"{self.spike.session_prefix}_{self._spike_sequence}"
        prompt = max(1, int(self.spike.prompt_tokens_mean))
        max_tokens = max(1, int(self.spike.max_tokens_mean))
        return {
            "type": "chat",
            "session_id": session_id,
            "prompt_tokens": prompt,
            "max_tokens": max_tokens,
            "estimated_kv_bytes": (prompt + max_tokens) * self.bytes_per_token,
            "seq_id": session_id,
            "continuation_of": None,
            "model_alias": "spike",
            "priority": "high",
            "spike": True,
        }

    def generate(self, sim_time: float, rng: random.Random) -> dict | None:
        if self._spike_due(sim_time, rng):
            return self._emit_spike(sim_time)
        return self.baseline.generate(sim_time, rng)
