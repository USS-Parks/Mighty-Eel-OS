from __future__ import annotations


class GpuModel:
    def __init__(self, total_vram: float = 80e9, max_batch_size: int = 64,
                 base_step_time: float = 0.010, scaling_factor: float = 0.002) -> None:
        self.total_vram = total_vram
        self.used_vram = 0.0
        self.max_batch_size = max_batch_size
        self.base_step_time = base_step_time
        self.scaling_factor = scaling_factor

    def step_time(self, batch_size: int) -> float:
        return self.base_step_time + batch_size * self.scaling_factor

    def can_accommodate(self, kv_bytes: float) -> bool:
        return self.used_vram + kv_bytes <= self.total_vram

    def allocate_vram(self, kv_bytes: float) -> bool:
        if self.can_accommodate(kv_bytes):
            self.used_vram += kv_bytes
            return True
        return False

    def free_vram(self, kv_bytes: float) -> None:
        self.used_vram = max(0.0, self.used_vram - kv_bytes)

    def utilization(self) -> float:
        return self.used_vram / self.total_vram if self.total_vram > 0 else 0.0


class MultiGpuModel:
    def __init__(self, num_gpus: int = 4, vram_per_gpu: float = 80e9,
                 max_batch_size: int = 64, base_step_time: float = 0.010,
                 scaling_factor: float = 0.002, topology_cost: float = 1.0) -> None:
        self.gpus = [
            GpuModel(vram_per_gpu, max_batch_size, base_step_time, scaling_factor)
            for _ in range(num_gpus)
        ]
        self.topology_cost = topology_cost

    def step_time(self, batch_size: int) -> float:
        per_gpu = self.gpus[0].step_time(batch_size)
        return per_gpu * self.topology_cost

    def can_accommodate(self, kv_bytes: float, num_gpus: int = 1) -> bool:
        needed_per_gpu = kv_bytes / num_gpus
        return all(g.can_accommodate(needed_per_gpu) for g in self.gpus[:num_gpus])

    def allocate_vram(self, kv_bytes: float, num_gpus: int = 1) -> bool:
        needed_per_gpu = kv_bytes / num_gpus
        return all(g.allocate_vram(needed_per_gpu) for g in self.gpus[:num_gpus])

    def free_vram(self, kv_bytes: float, num_gpus: int = 1) -> None:
        per_gpu = kv_bytes / num_gpus
        for g in self.gpus[:num_gpus]:
            g.free_vram(per_gpu)

    def utilization(self) -> float:
        return sum(g.utilization() for g in self.gpus) / len(self.gpus)
