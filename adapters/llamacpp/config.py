"""llama.cpp adapter configuration.

Default values assume local llama-server (llama.cpp HTTP server).
Critical for fallback, lightweight deployments, and GGUF models.

"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class LlamaCppConfig:
    """Configuration for the llama.cpp adapter.

    Loaded from adapter section of product tier config (TOML).
    llama-server exposes an OpenAI-compatible API on localhost.
    """

    # Connection
    host: str = "127.0.0.1"
    port: int = 8080
    timeout_ms: int = 30000
    stream_timeout_ms: int = 120000

    # Model defaults
    default_model: str = "llama3.1-8b-instruct-q4_K_M.gguf"
    model_path: str = "/var/lib/mai/models"

    # GPU offload layers (-1 = auto, 0 = CPU only, N = N layers)
    n_gpu_layers: int = -1

    # Context size
    context_size: int = 8192
    max_context_size: int = 32768

    # Memory mapping
    use_mmap: bool = True
    use_mlock: bool = False

    # Threading
    n_threads: int = -1  # -1 = auto-detect
    n_threads_batch: int = -1

    # Grammar (GBNF format path or inline)
    default_grammar: str | None = None

    # Metal backend (Apple Silicon)
    use_metal: bool = False

    # Concurrency
    max_concurrent_requests: int = 4
    n_parallel: int = 1  # Number of parallel sequences

    # Health check
    health_check_timeout_ms: int = 5000

    # Additional options
    extra_options: dict[str, object] = field(default_factory=dict)

    @property
    def base_url(self) -> str:
        """Construct base URL for llama-server."""
        return f"http://{self.host}:{self.port}"

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> LlamaCppConfig:
        """Create config from a dictionary (TOML section)."""
        known_fields = {f.name for f in cls.__dataclass_fields__.values()}
        known = {k: v for k, v in data.items() if k in known_fields}
        extra = {k: v for k, v in data.items() if k not in known_fields}
        config = cls(**known)  # type: ignore[arg-type]
        config.extra_options.update(extra)
        return config
