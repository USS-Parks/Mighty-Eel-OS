"""Ollama adapter configuration.

Default values assume local Ollama server (air-gapped deployment).
All network access is localhost-only by design.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class OllamaConfig:
    """Configuration for the Ollama adapter.

    Loaded from adapter section of product tier config (TOML).
    All fields have sane defaults for local single-GPU deployment.
    """

    # Connection
    host: str = "127.0.0.1"
    port: int = 11434
    timeout_ms: int = 30000
    stream_timeout_ms: int = 120000

    # Model defaults
    default_model: str = "llama3.1:8b-instruct-q4_K_M"
    embedding_model: str = "nomic-embed-text"

    # GPU layer assignment (Ollama num_gpu parameter)
    # -1 = auto (let Ollama decide), 0 = CPU only, N = N layers on GPU
    num_gpu_layers: int = -1

    # Keep-alive duration for loaded models (Ollama keep_alive)
    # "5m" = 5 minutes, "0" = unload immediately, "-1" = keep forever
    keep_alive: str = "5m"

    # Concurrency
    max_concurrent_requests: int = 4

    # Health check
    health_check_timeout_ms: int = 5000

    # Model pull settings (for offline/air-gapped: pulls are disabled)
    allow_pull: bool = False

    # NUMA support for multi-socket systems
    numa: bool = False

    # Additional Ollama-specific options passed through
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for Ollama API."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> OllamaConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
