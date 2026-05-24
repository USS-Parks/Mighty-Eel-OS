from __future__ import annotations

import random
from typing import Protocol


class WorkloadGenerator(Protocol):
    def generate(self, sim_time: float, rng: random.Random) -> dict | None: ...


class ChatWorkload:
    def __init__(self, arrival_rate: float = 2.0, session_length_mean: float = 5.0,
                 session_length_std: float = 2.0, prompt_tokens_mean: float = 200,
                 prompt_tokens_std: float = 100, max_tokens_mean: float = 100,
                 max_tokens_std: float = 50, reuse_rate: float = 0.3) -> None:
        self.arrival_rate = arrival_rate
        self.session_length_mean = session_length_mean
        self.session_length_std = session_length_std
        self.prompt_tokens_mean = prompt_tokens_mean
        self.prompt_tokens_std = prompt_tokens_std
        self.max_tokens_mean = max_tokens_mean
        self.max_tokens_std = max_tokens_std
        self.reuse_rate = reuse_rate
        self._active_sessions: dict[str, int] = {}

    def generate(self, sim_time: float, rng: random.Random) -> dict | None:
        if rng.random() > self.arrival_rate / 10.0:
            return None
        session_id = f"chat_{sim_time:.0f}_{rng.randint(0, 9999)}"
        progress = 0
        if self._active_sessions and rng.random() < self.reuse_rate:
            session_id = rng.choice(list(self._active_sessions.keys()))
            progress = self._active_sessions[session_id]
            if progress >= self.session_length_mean + self.session_length_std:
                del self._active_sessions[session_id]
                return None
        max(1, int(self.session_length_mean - progress))
        self._active_sessions[session_id] = progress + 1
        prompt_tokens = max(10, int(rng.gauss(self.prompt_tokens_mean, self.prompt_tokens_std)))
        max_tokens = max(10, int(rng.gauss(self.max_tokens_mean, self.max_tokens_std)))
        estimated_kv_bytes = (prompt_tokens + max_tokens) * 2.0 * 1024
        return {
            "type": "chat",
            "session_id": session_id,
            "prompt_tokens": prompt_tokens,
            "max_tokens": max_tokens,
            "estimated_kv_bytes": estimated_kv_bytes,
            "seq_id": f"seq_{session_id}_{progress}",
            "continuation_of": session_id if progress > 0 else None,
        }


class BatchWorkload:
    def __init__(self, arrival_rate: float = 0.5, prompt_tokens_mean: float = 4000,
                 prompt_tokens_std: float = 2000, max_tokens_mean: float = 500,
                 max_tokens_std: float = 200) -> None:
        self.arrival_rate = arrival_rate
        self.prompt_tokens_mean = prompt_tokens_mean
        self.prompt_tokens_std = prompt_tokens_std
        self.max_tokens_mean = max_tokens_mean
        self.max_tokens_std = max_tokens_std

    def generate(self, sim_time: float, rng: random.Random) -> dict | None:
        if rng.random() > self.arrival_rate / 5.0:
            return None
        prompt_tokens = max(100, int(rng.gauss(self.prompt_tokens_mean, self.prompt_tokens_std)))
        max_tokens = max(50, int(rng.gauss(self.max_tokens_mean, self.max_tokens_std)))
        estimated_kv_bytes = (prompt_tokens + max_tokens) * 2.0 * 1024
        return {
            "type": "batch",
            "session_id": f"batch_{sim_time:.0f}_{rng.randint(0, 9999)}",
            "prompt_tokens": prompt_tokens,
            "max_tokens": max_tokens,
            "estimated_kv_bytes": estimated_kv_bytes,
            "seq_id": f"seq_batch_{sim_time:.0f}_{rng.randint(0, 9999)}",
            "continuation_of": None,
        }


class MixedWorkload:
    def __init__(self, chat_ratio: float = 0.7, chat_config: dict | None = None,
                 batch_config: dict | None = None, arrival_rate: float = 1.5) -> None:
        self.chat_ratio = chat_ratio
        self.arrival_rate = arrival_rate
        self.chat_workload = ChatWorkload(**(chat_config or {}))
        self.batch_workload = BatchWorkload(**(batch_config or {}))

    def generate(self, sim_time: float, rng: random.Random) -> dict | None:
        if rng.random() > self.arrival_rate / 8.0:
            return None
        if rng.random() < self.chat_ratio:
            return self.chat_workload.generate(sim_time, rng)
        return self.batch_workload.generate(sim_time, rng)
