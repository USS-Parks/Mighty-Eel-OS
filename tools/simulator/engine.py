from __future__ import annotations

import heapq
import random
from typing import Protocol


class SimulationEvent:
    def __init__(self, time: float, kind: str, data: dict | None = None) -> None:
        self.time = time
        self.kind = kind
        self.data = data or {}

    def __lt__(self, other: SimulationEvent) -> bool:
        return self.time < other.time

    def __repr__(self) -> str:
        return f"Event({self.time:.3f}, {self.kind})"


class RequestGenerator(Protocol):
    def generate(self, sim_time: float) -> dict: ...


class Scheduler(Protocol):
    def select_instance(self, request: dict, instances: list[dict]) -> str | None: ...


class KvManager(Protocol):
    def allocate(self, seq_id: str, tokens: int, kv_bytes: float) -> bool: ...
    def deallocate(self, seq_id: str) -> None: ...
    def eviction_candidates(self, needed_bytes: float) -> list[tuple[str, float, float]]: ...
    def evict(self, seq_ids: list[str]) -> float: ...
    def can_fit(self, estimated_tokens: int, model_factor: float) -> bool: ...
    def free_bytes(self) -> float: ...
    def total_bytes(self) -> float: ...
    def touch(self, seq_id: str) -> None: ...


class GpuModel(Protocol):
    def step_time(self, batch_size: int) -> float: ...
    def can_accommodate(self, kv_bytes: float) -> bool: ...


class SimEngine:
    def __init__(self, seed: int = 42) -> None:
        self.rng = random.Random(seed)
        self.current_time: float = 0.0
        self.event_queue: list[SimulationEvent] = []
        self.request_generator: RequestGenerator | None = None
        self.scheduler: Scheduler | None = None
        self.kv_manager: KvManager | None = None
        self.gpu_model: GpuModel | None = None
        self.instances: list[dict] = []
        self.results: list[dict] = []
        self._running = True
        self.active_sequences: dict[str, dict] = {}
        self.pending_requests: list[dict] = []

    def schedule_event(self, event: SimulationEvent) -> None:
        heapq.heappush(self.event_queue, event)

    def run(self, max_time: float = 100.0, max_events: int = 100000) -> list[dict]:
        self._running = True
        events_processed = 0
        while self.event_queue and self._running and self.current_time < max_time:
            if events_processed >= max_events:
                break
            event = heapq.heappop(self.event_queue)
            self.current_time = event.time
            self._dispatch(event)
            events_processed += 1
        return self.results

    def stop(self) -> None:
        self._running = False

    def _dispatch(self, event: SimulationEvent) -> None:
        if self.kv_manager and hasattr(self.kv_manager, 'set_time'):
            self.kv_manager.set_time(self.current_time)
        handler = getattr(self, f"_on_{event.kind}", None)
        if handler is not None:
            handler(event)
        else:
            raise ValueError(f"No handler for event kind: {event.kind}")

    def _estimate_duration(self, req: dict) -> float:
        prompt_tokens = req.get("prompt_tokens", 100)
        max_tokens = req.get("max_tokens", 100)
        total_tokens = prompt_tokens + max_tokens
        step_time = self.gpu_model.step_time(1) if self.gpu_model else 0.01
        tokens_per_second = 512 / step_time if step_time > 0 else 10000
        return total_tokens / tokens_per_second

    def _on_request_arrive(self, event: SimulationEvent) -> None:
        req = self.request_generator.generate(self.current_time) if self.request_generator else event.data
        if req is None or req.get("type") == "none":
            return
        instance_id = self.scheduler.select_instance(req, self.instances) if self.scheduler else "default"
        if instance_id is None:
            return
        seq_id = req.get("seq_id", f"seq_{self.current_time}_{self.rng.randint(0, 99999)}")
        kv_bytes = req.get("estimated_kv_bytes", 1.0)
        total_tokens = req.get("prompt_tokens", 100) + req.get("max_tokens", 100)

        while self.kv_manager and not self.kv_manager.can_fit(total_tokens, 1.0):
            needed = kv_bytes * 2
            candidates = self.kv_manager.eviction_candidates(needed)
            if not candidates:
                self.pending_requests.append(req)
                return
            to_evict = [c[0] for c in candidates]
            self.kv_manager.evict(to_evict)
            self.results.append({
                "time": self.current_time, "kind": "eviction",
                "seq_ids": to_evict, "reason": "admission",
            })

        if self.kv_manager and not self.kv_manager.allocate(seq_id, total_tokens, kv_bytes):
            self.pending_requests.append(req)
            return

        self.active_sequences[seq_id] = {
            "seq_id": seq_id, "instance_id": instance_id, "request": req,
            "kv_bytes": kv_bytes, "arrived_at": self.current_time,
        }
        self.results.append({
            "time": self.current_time, "kind": "admission",
            "seq_id": seq_id, "instance_id": instance_id,
        })

        duration = self._estimate_duration(req)
        self.schedule_event(SimulationEvent(self.current_time + duration, "request_complete", {
            "seq_id": seq_id, "instance_id": instance_id, "request": req,
        }))

    def _on_request_complete(self, event: SimulationEvent) -> None:
        seq_id = event.data.get("seq_id", "")
        if seq_id not in self.active_sequences:
            return
        seq = self.active_sequences[seq_id]
        elapsed = self.current_time - seq["arrived_at"]
        total_tokens = seq["request"].get("prompt_tokens", 100) + seq["request"].get("max_tokens", 100)
        self.results.append({
            "time": self.current_time, "kind": "completed",
            "seq_id": seq_id, "instance_id": seq["instance_id"],
            "latency_ms": elapsed * 1000,
            "tokens_generated": total_tokens,
            "token_rate": total_tokens / max(elapsed, 0.001),
        })
        if self.kv_manager:
            self.kv_manager.deallocate(seq_id)
        del self.active_sequences[seq_id]

        admitted = []
        for req in list(self.pending_requests):
            seq_id2 = req.get("seq_id", f"seq_{self.current_time}_{self.rng.randint(0, 99999)}")
            kv_bytes2 = req.get("estimated_kv_bytes", 1.0)
            total_tokens2 = req.get("prompt_tokens", 100) + req.get("max_tokens", 100)
            if self.kv_manager and self.kv_manager.can_fit(total_tokens2, 1.0):
                if self.kv_manager.allocate(seq_id2, total_tokens2, kv_bytes2):
                    self.active_sequences[seq_id2] = {
                        "seq_id": seq_id2, "instance_id": "default",
                        "request": req, "kv_bytes": kv_bytes2,
                        "arrived_at": self.current_time,
                    }
                    self.results.append({
                        "time": self.current_time, "kind": "admission",
                        "seq_id": seq_id2, "from_pending": True,
                    })
                    duration = self._estimate_duration(req)
                    self.schedule_event(SimulationEvent(
                        self.current_time + duration, "request_complete",
                        {"seq_id": seq_id2, "request": req},
                    ))
                    admitted.append(req)
        for r in admitted:
            self.pending_requests.remove(r)
