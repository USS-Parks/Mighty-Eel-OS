from __future__ import annotations

from typing import Protocol


class KvPolicy(Protocol):
    def allocate(self, seq_id: str, tokens: int, kv_bytes: float) -> bool: ...
    def deallocate(self, seq_id: str) -> None: ...
    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]: ...
    def evict(self, seq_ids: list[str]) -> float: ...
    def can_fit(self, estimated_tokens: int, model_factor: float) -> bool: ...
    def free_bytes(self) -> float: ...
    def total_bytes(self) -> float: ...
    def touch(self, seq_id: str) -> None: ...


class BaseKvManager:
    def __init__(self, total_bytes: float = 80e9, min_residency_time: float = 0.0,
                 recently_evicted_penalty: float = 10.0) -> None:
        self.total_bytes = total_bytes
        self.used_bytes = 0.0
        self.sequences: dict[str, dict] = {}
        self.access_times: dict[str, float] = {}
        self.current_time: float = 0.0
        self.min_residency_time = min_residency_time
        self.recently_evicted_penalty = recently_evicted_penalty

    def allocate(self, seq_id: str, tokens: int, kv_bytes: float) -> bool:
        if self.used_bytes + kv_bytes > self.total_bytes:
            return False
        self.sequences[seq_id] = {"tokens": tokens, "kv_bytes": kv_bytes, "created_at": self.current_time}
        self.access_times[seq_id] = self.current_time
        self.used_bytes += kv_bytes
        return True

    def deallocate(self, seq_id: str) -> None:
        if seq_id in self.sequences:
            self.used_bytes -= self.sequences[seq_id]["kv_bytes"]
            del self.sequences[seq_id]
            self.access_times.pop(seq_id, None)

    def can_fit(self, estimated_tokens: int, model_factor: float) -> bool:
        estimated_bytes = estimated_tokens * model_factor * 2.0 * 1024
        return self.used_bytes + estimated_bytes <= self.total_bytes

    def free_bytes(self) -> float:
        return self.total_bytes - self.used_bytes

    def touch(self, seq_id: str) -> None:
        if seq_id in self.access_times:
            self.access_times[seq_id] = self.current_time

    def set_time(self, time: float) -> None:
        self.current_time = time


class LruKvManager(BaseKvManager):
    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]:
        sorted_seqs = sorted(self.access_times.items(), key=lambda x: x[1])
        candidates: list[tuple[str, float, float]] = []
        for seq_id, last_access in sorted_seqs:
            if seq_id not in self.sequences:
                continue
            idle = self.current_time - last_access
            if idle < self.min_residency_time:
                continue
            kv = self.sequences[seq_id]["kv_bytes"]
            candidates.append((seq_id, kv, self.current_time - last_access))
        return candidates

    def evict(self, seq_ids: list[str]) -> float:
        freed = 0.0
        for sid in seq_ids:
            if sid in self.sequences:
                freed += self.sequences[sid]["kv_bytes"]
                self.deallocate(sid)
        return freed


class SizeBasedKvManager(BaseKvManager):
    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]:
        sorted_seqs = sorted(self.sequences.items(), key=lambda x: -x[1]["kv_bytes"])
        candidates: list[tuple[str, float, float]] = []
        for seq_id, info in sorted_seqs:
            idle = self.current_time - self.access_times.get(seq_id, 0)
            if idle < self.min_residency_time:
                continue
            candidates.append((seq_id, info["kv_bytes"], info["kv_bytes"]))
        return candidates

    def evict(self, seq_ids: list[str]) -> float:
        freed = 0.0
        for sid in seq_ids:
            if sid in self.sequences:
                freed += self.sequences[sid]["kv_bytes"]
                self.deallocate(sid)
        return freed


class HeuristicScoredKvManager(BaseKvManager):
    def __init__(self, total_bytes: float = 80e9, weight_idle: float = 0.4,
                 weight_size: float = 0.3, weight_priority: float = 0.3,
                 min_residency_time: float = 30.0, recently_evicted_penalty: float = 10.0) -> None:
        super().__init__(total_bytes, min_residency_time, recently_evicted_penalty)
        self.weight_idle = weight_idle
        self.weight_size = weight_size
        self.weight_priority = weight_priority
        self.eviction_history: dict[str, float] = {}

    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]:
        candidates: list[tuple[str, float, float]] = []
        for seq_id, info in self.sequences.items():
            idle = self.current_time - self.access_times.get(seq_id, 0)
            if idle < self.min_residency_time:
                continue
            idle_norm = idle / (idle + 60.0)
            size_norm = info["kv_bytes"] / self.total_bytes
            priority = info.get("priority", 0.5)
            score = (self.weight_idle * idle_norm + self.weight_size * size_norm
                     - self.weight_priority * priority)
            if seq_id in self.eviction_history:
                last_evicted = self.current_time - self.eviction_history[seq_id]
                if last_evicted < self.recently_evicted_penalty:
                    score += 0.5
            candidates.append((seq_id, info["kv_bytes"], score))
        candidates.sort(key=lambda x: -x[2])
        return candidates

    def evict(self, seq_ids: list[str]) -> float:
        freed = 0.0
        for sid in seq_ids:
            if sid in self.sequences:
                freed += self.sequences[sid]["kv_bytes"]
                self.eviction_history[sid] = self.current_time
                self.deallocate(sid)
        return freed


class BatchAwareKvManager(HeuristicScoredKvManager):
    def __init__(self, total_bytes: float = 80e9, weight_idle: float = 0.3,
                 weight_size: float = 0.2, weight_priority: float = 0.2,
                 weight_batch: float = 0.3, min_residency_time: float = 30.0,
                 recently_evicted_penalty: float = 10.0) -> None:
        super().__init__(total_bytes, weight_idle, weight_size, weight_priority,
                         min_residency_time, recently_evicted_penalty)
        self.weight_batch = weight_batch
        self.active_batch: set[str] = set()

    def set_active_batch(self, seq_ids: set[str]) -> None:
        self.active_batch = seq_ids

    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]:
        candidates = super().eviction_candidates(needed_bytes)
        adjusted: list[tuple[str, float, float]] = []
        for seq_id, kv_bytes, score in candidates:
            if seq_id in self.active_batch:
                score -= self.weight_batch * 2.0
            adjusted.append((seq_id, kv_bytes, score))
        adjusted.sort(key=lambda x: -x[2])
        return adjusted
