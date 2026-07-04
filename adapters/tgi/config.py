"""TGI (Text Generation Inference) adapter configuration.

Default values assume local HuggingFace TGI server.
Supports quantization, speculative decoding, and watermarking.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class TgiConfig:
    """Configuration for the TGI adapter."""

    # Connection
    host: str = "127.0.0.1"
    port: int = 8080
    timeout_ms: int = 30000
    stream_timeout_ms: int = 180000

    # Model
    default_model: str = ""  # TGI serves a single model per instance

    # Quantization
    quantize: str | None = None  # bitsandbytes, gptq, awq, eetq, fp8

    # Speculative decoding
    speculate: int | None = None  # Number of speculative tokens

    # Watermarking (for compliance audit trails)
    watermark: bool = False

    # Flash Attention
    flash_attention: bool = True

    # Batching
    max_batch_total_tokens: int = 32768
    max_waiting_tokens: int = 20
    max_concurrent_requests: int = 128

    # Limits
    max_input_tokens: int = 4096
    max_total_tokens: int = 8192

    # Health check
    health_check_timeout_ms: int = 5000

    # Additional TGI options
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for TGI server."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> TgiConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
