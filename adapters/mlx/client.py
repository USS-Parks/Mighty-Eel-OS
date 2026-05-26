"""MLX in-process client.

Wraps the `mlx_lm` library so the adapter can talk to a uniform surface.
mlx-lm is imported lazily inside `load()` because the package only
installs on Apple Silicon — non-Apple CI must be able to import this
module without raising.

Three responsibilities:
  1. lazy-import + load model + tokenizer (one-shot, in `load`)
  2. provide `generate` and `stream_generate` shims that the adapter
     can call without knowing mlx-lm's exact signature
  3. provide `close` that releases handles in an idempotent way

The client never calls the network. The trust boundary is the operator's
filesystem path supplied via `MLXConfig.model_path`.

Session J-25 (DOUGHERTY lane) deliverable.
"""

from __future__ import annotations

import logging
import platform
from collections.abc import Iterator
from typing import Any

logger = logging.getLogger("mai.adapters.mlx.client")


class MLXLoadError(RuntimeError):
    """Raised when mlx-lm cannot be imported or the model cannot be loaded.

    The adapter maps this into `BackendUnavailableError` (mlx-lm absent or
    wrong platform) or `ModelNotFoundError` (path does not resolve)
    depending on the cause.
    """


def is_apple_silicon() -> bool:
    """True when the runtime is macOS on arm64.

    mlx-lm only works on Apple Silicon. We check both `system()` and
    `machine()` because GitHub Actions exposes `darwin` runners on
    Intel hardware, and tests must skip cleanly there.
    """
    return platform.system() == "Darwin" and platform.machine() in {"arm64", "aarch64"}


def _tokenizer_config(model_path: str, tokenizer_path: str) -> dict[str, str] | None:
    if tokenizer_path == model_path:
        return None
    return {"path": tokenizer_path}


def _load_model_handles(mlx_lm: Any, model_path: str, tokenizer_path: str) -> tuple[Any, Any]:
    return mlx_lm.load(
        model_path,
        tokenizer_config=_tokenizer_config(model_path, tokenizer_path),
    )


def _wrong_platform_message() -> str:
    return (
        "MLX requires Apple Silicon (Darwin/arm64); "
        f"runtime is {platform.system()}/{platform.machine()}"
    )


def _chunk_text(chunk: Any) -> str:
    if isinstance(chunk, str):
        return chunk
    if hasattr(chunk, "text"):
        return chunk.text
    return str(chunk)


class MLXClient:
    """In-process MLX client.

    Lifecycle:
      __init__ stores config only.
      load() imports mlx_lm, validates the path, loads model+tokenizer.
      generate() / stream_generate() use the loaded handles.
      close() releases the handles and marks the client unloaded.

    Tests can substitute a fake `_mlx_lm` module (via the constructor's
    `mlx_module` kwarg) so unit tests run on non-Apple CI.
    """

    def __init__(
        self,
        model_path: str,
        tokenizer_path: str = "",
        *,
        mlx_module: Any | None = None,
    ) -> None:
        self.model_path = model_path
        self.tokenizer_path = tokenizer_path or model_path
        self._mlx_lm: Any | None = mlx_module
        self._model: Any | None = None
        self._tokenizer: Any | None = None
        self._loaded: bool = False
        self._backend_version: str = "unknown"

    @property
    def loaded(self) -> bool:
        return self._loaded

    @property
    def backend_version(self) -> str:
        return self._backend_version

    def load(self) -> None:
        """Lazy-import mlx-lm and load the model+tokenizer.

        Raises:
          MLXLoadError: when mlx-lm cannot be imported, when the
            platform is wrong, or when the model path cannot be loaded.
        """
        if self._loaded:
            return

        mlx_lm = self._ensure_mlx_module()

        if not self.model_path:
            raise MLXLoadError("model_path is empty; nothing to load")

        try:
            handles = _load_model_handles(mlx_lm, self.model_path, self.tokenizer_path)
            self._model, self._tokenizer = handles
        except FileNotFoundError as e:
            raise MLXLoadError(f"model path not found: {self.model_path}") from e
        except Exception as e:
            raise MLXLoadError(f"mlx-lm load failed: {e}") from e

        self._backend_version = getattr(mlx_lm, "__version__", "unknown")
        self._loaded = True
        logger.info(
            "MLX client loaded: path=%s version=%s",
            self.model_path,
            self._backend_version,
        )

    def _ensure_mlx_module(self) -> Any:
        """Return the injected or lazily imported mlx_lm module."""
        if self._mlx_lm is not None:
            return self._mlx_lm
        if not is_apple_silicon():
            raise MLXLoadError(_wrong_platform_message())
        try:
            import mlx_lm  # type: ignore[import-not-found]
        except ImportError as e:
            raise MLXLoadError(f"mlx-lm not installed: {e}") from e
        self._mlx_lm = mlx_lm
        return self._mlx_lm

    def generate(
        self,
        prompt: str,
        *,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> tuple[str, int, bool]:
        """Run a non-streaming generation.

        Returns (text, tokens_generated, hit_max).
        `hit_max` lets the adapter map FinishReason.MAX_TOKENS without
        re-counting tokens — mlx-lm itself does not expose a finish
        reason in stable form.
        """
        if not self._loaded or self._mlx_lm is None:
            raise MLXLoadError("client not loaded")

        text = self._mlx_lm.generate(
            self._model,
            self._tokenizer,
            prompt=prompt,
            max_tokens=max_tokens,
            temp=temperature,
            top_p=top_p,
        )
        # mlx-lm returns the completion string. Estimate tokens via
        # the tokenizer when possible; fall back to ceil(len/4).
        tokens = self._estimate_tokens(text)
        hit_max = tokens >= max_tokens
        return text, tokens, hit_max

    def stream_generate(
        self,
        prompt: str,
        *,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> Iterator[str]:
        """Yield generated text chunks in order.

        mlx-lm's `stream_generate` yields decoded chunks (string deltas
        in newer versions, token-object-with-text in older). We
        normalize both shapes into plain `str` chunks.
        """
        if not self._loaded or self._mlx_lm is None:
            raise MLXLoadError("client not loaded")

        for chunk in self._mlx_lm.stream_generate(
            self._model,
            self._tokenizer,
            prompt=prompt,
            max_tokens=max_tokens,
            temp=temperature,
            top_p=top_p,
        ):
            yield _chunk_text(chunk)

    def _estimate_tokens(self, text: str) -> int:
        """Best-effort token count using the loaded tokenizer."""
        if self._tokenizer is None:
            return max(1, len(text) // 4)
        try:
            encoded = self._tokenizer.encode(text)
            return len(encoded)
        except Exception:
            return max(1, len(text) // 4)

    def close(self) -> None:
        """Idempotent release of model/tokenizer handles.

        MLX-lm does not expose an explicit free; dropping references
        lets the Python GC reclaim the backing arrays.
        """
        self._model = None
        self._tokenizer = None
        self._loaded = False
