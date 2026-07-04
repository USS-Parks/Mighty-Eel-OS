"""ExLlamaV2 adapter configuration.

Default values assume local ExLlamaV2 server (TabbyAPI or custom).
Specializes in EXL2/GPTQ quantized models with multi-model multiplexing.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class ExLlamaV2Config:
    """Configuration for the ExLlamaV2 adapter."""

    # Connection
    host: str = "127.0.0.1"
    port: int = 5000
    timeout_ms: int = 30000
    stream_timeout_ms: int = 120000

    # Model
    default_model: str = ""
    model_dir: str = "/var/lib/mai/models"

    # Quantization
    quantization: str = "exl2"  # exl2 or gptq

    # Cache (paged cache for memory efficiency)
    cache_mode: str = "Q4"  # FP16, Q8, Q4
    max_seq_len: int = 8192
    cache_size: int = 8192  # Max KV cache size

    # Multi-model multiplexing
    max_loaded_models: int = 2
    auto_unload: bool = True

    # Dynamic batching
    max_batch_size: int = 16

    # GPU
    gpu_split: list[float] = field(default_factory=list)  # VRAM split ratios

    # Concurrency
    max_concurrent_requests: int = 16

    # Health check
    health_check_timeout_ms: int = 5000

    # Additional options
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for ExLlamaV2 server."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> ExLlamaV2Config:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
