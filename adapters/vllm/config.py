"""vLLM adapter configuration.

Default values assume local vLLM server with OpenAI-compatible API.
vLLM is the primary adapter for Ranger/Pack Leader tiers (tensor parallelism).

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class VllmConfig:
    """Configuration for the vLLM adapter.

    Loaded from adapter section of product tier config (TOML).
    vLLM serves OpenAI-compatible API natively.
    """

    # Connection
    host: str = "127.0.0.1"
    port: int = 8000
    timeout_ms: int = 30000
    stream_timeout_ms: int = 180000

    # Model defaults
    default_model: str = "Qwen/Qwen3-70B-AWQ"

    # Tensor parallelism (multi-GPU)
    tensor_parallel_size: int = 1

    # LoRA configuration
    enable_lora: bool = False
    max_lora_rank: int = 64
    lora_modules: list[str] = field(default_factory=list)

    # Batching
    max_num_seqs: int = 256
    max_num_batched_tokens: int = 32768

    # Speculative decoding
    speculative_model: str | None = None
    num_speculative_tokens: int = 5

    # Quantization
    quantization: str | None = None  # awq, gptq, squeezellm, fp8

    # GPU memory utilization (fraction, 0.0-1.0)
    gpu_memory_utilization: float = 0.90

    # Structured output
    guided_decoding_backend: str = "outlines"  # outlines or lm-format-enforcer

    # Concurrency
    max_concurrent_requests: int = 64

    # Health check
    health_check_timeout_ms: int = 5000

    # Additional vLLM-specific options passed through
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for vLLM OpenAI-compatible API."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> VllmConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
