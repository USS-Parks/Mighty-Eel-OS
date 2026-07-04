"""TensorRT-LLM adapter configuration.

Default values assume local NVIDIA Triton Inference Server with TensorRT-LLM backend.
Highest performance target for H100/H200 hardware.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class TensorRtConfig:
    """Configuration for the TensorRT-LLM adapter."""

    # Connection (Triton Inference Server)
    host: str = "127.0.0.1"
    port: int = 8001  # Triton HTTP port
    grpc_port: int = 8002  # Triton gRPC port
    timeout_ms: int = 60000
    stream_timeout_ms: int = 300000

    # Model
    default_model: str = "ensemble"  # Triton model name
    engine_dir: str = "/var/lib/mai/engines"

    # Engine build settings
    max_batch_size: int = 64
    max_input_len: int = 4096
    max_output_len: int = 4096
    max_beam_width: int = 1

    # Precision
    precision: str = "fp16"  # fp16, fp8, int8

    # Multi-GPU
    tensor_parallel_size: int = 1
    pipeline_parallel_size: int = 1

    # Inflight batching
    enable_inflight_batching: bool = True
    max_num_sequences: int = 128

    # KV cache
    kv_cache_free_gpu_mem_fraction: float = 0.85

    # Concurrency
    max_concurrent_requests: int = 128

    # Health check
    health_check_timeout_ms: int = 10000

    # Additional options
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for Triton HTTP API."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> TensorRtConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
