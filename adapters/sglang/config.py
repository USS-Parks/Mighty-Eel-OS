"""SGLang adapter configuration.

Default values assume local SGLang server. Specializes in
RadixAttention KV cache reuse and constrained decoding with
guaranteed schema compliance.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class SglangConfig:
    """Configuration for the SGLang adapter."""

    # Connection
    host: str = "127.0.0.1"
    port: int = 30000
    timeout_ms: int = 30000
    stream_timeout_ms: int = 180000

    # Model
    default_model: str = ""

    # RadixAttention
    enable_radix_attention: bool = True
    radix_cache_size: int = 65536  # Max number of cached prefix tokens

    # Constrained decoding
    default_constrained_backend: str = "outlines"  # outlines or xgrammar

    # Fork-based parallelism
    max_forks: int = 8

    # Batching
    max_running_requests: int = 128
    max_total_tokens: int = 65536

    # Vision model support
    enable_vision: bool = False

    # Concurrency
    max_concurrent_requests: int = 64

    # Health check
    health_check_timeout_ms: int = 5000

    # Additional options
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for SGLang server."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> SglangConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
