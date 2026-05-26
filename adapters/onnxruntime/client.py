"""ONNX Runtime client boundary.

Heavy runtime imports are delayed until load time so other adapter
consumers can import this package without native ONNX wheels.
"""

from __future__ import annotations

import logging
import os
from collections.abc import Iterator
from typing import Any

from adapters.onnxruntime.client_helpers import (
    generate_genai_chunks,
    safe_import,
)
from adapters.onnxruntime.types import (
    LoadedModelInfo,
    OnnxRuntimeClientError,
    OnnxStreamChunk,
)

logger = logging.getLogger("mai.adapters.onnxruntime.client")


class OnnxRuntimeClient:
    """In-process ONNX Runtime client.

    Lifecycle:
        c = OnnxRuntimeClient(model_path=..., tokenizer_path=..., providers=...)
        info = c.load(embedding_only=False)
        c.close()
    """

    def __init__(
        self,
        model_path: str,
        tokenizer_path: str,
        providers: list[str],
    ) -> None:
        self._model_path = model_path
        self._tokenizer_path = tokenizer_path
        self._providers = list(providers)
        self._genai_model: Any = None
        self._genai_tokenizer: Any = None
        self._genai_module: Any = None
        self._ort_session: Any = None
        self._ort_module: Any = None
        self._closed = False
        self._info: LoadedModelInfo | None = None

    # ── load / close ────────────────────────────────────────────────────────

    def load(self, *, embedding_only: bool) -> LoadedModelInfo:
        """Resolve providers, import the right runtime, and load the model.

        Raises :class:`OnnxRuntimeClientError` with one of these kinds:
            "ValidationError"      — bad path / empty config
            "BackendUnavailable"   — onnxruntime not importable / unloadable
            "ModelNotFound"        — model path missing
            "OutOfMemory"          — runtime reported allocation failure
        """
        if not self._model_path:
            raise OnnxRuntimeClientError(
                "ValidationError", "OnnxRuntimeConfig.model_path is required",
            )
        if not os.path.exists(self._model_path):
            raise OnnxRuntimeClientError(
                "ModelNotFound", f"model path does not exist: {self._model_path}",
            )

        ort_module = safe_import("onnxruntime")
        if ort_module is None:
            raise OnnxRuntimeClientError(
                "BackendUnavailable",
                "onnxruntime is not installed in this Python environment",
            )
        self._ort_module = ort_module

        if embedding_only:
            self._load_inference_session()
            self._info = LoadedModelInfo(
                supports_generation=False,
                supports_embedding=True,
                backend="session",
                backend_version=str(getattr(ort_module, "__version__", "unknown")),
                model_id=os.path.basename(self._model_path.rstrip(os.sep)) or "onnx-model",
            )
            return self._info

        # Prefer onnxruntime-genai for generation. Fall back to plain
        # session if the package is missing — generation will then be
        # unsupported and the adapter will report it honestly.
        genai_module = safe_import("onnxruntime_genai")
        if genai_module is not None:
            self._load_genai(genai_module)
            self._info = LoadedModelInfo(
                supports_generation=True,
                supports_embedding=False,
                backend="genai",
                backend_version=str(
                    getattr(genai_module, "__version__", "unknown"),
                ),
                model_id=os.path.basename(self._model_path.rstrip(os.sep))
                or "onnx-model",
            )
            return self._info

        # onnxruntime-genai not available — degrade to session.
        self._load_inference_session()
        self._info = LoadedModelInfo(
            supports_generation=False,
            supports_embedding=False,
            backend="session",
            backend_version=str(getattr(ort_module, "__version__", "unknown")),
            model_id=os.path.basename(self._model_path.rstrip(os.sep)) or "onnx-model",
        )
        return self._info

    def close(self) -> None:
        """Idempotent shutdown."""
        self._closed = True
        self._genai_model = None
        self._genai_tokenizer = None
        self._ort_session = None
        # Module refs deliberately retained — releasing the modules
        # would force a full re-import on the next load() and changes
        # nothing about resource cleanup.

    # ── generation ──────────────────────────────────────────────────────────

    def generate_once(
        self,
        prompt: str,
        *,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> tuple[str, int]:
        """Run autoregressive generation to completion. Returns (text, n_tokens).

        Raises :class:`OnnxRuntimeClientError` with kind ``UnsupportedOperation``
        when the loaded backend is not generation-capable.
        """
        self._require_generation_ready()
        chunks: list[str] = []
        count = 0
        for chunk in self.generate_stream(
            prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
        ):
            chunks.append(chunk.text)
            if not chunk.is_final:
                count += 1
        return "".join(chunks), count

    def generate_stream(
        self,
        prompt: str,
        *,
        max_tokens: int,
        temperature: float,
        top_p: float,
    ) -> Iterator[OnnxStreamChunk]:
        """Yield one OnnxStreamChunk per decoded token, then a terminator."""
        self._require_generation_ready()
        genai = self._genai_module
        assert genai is not None
        assert self._genai_model is not None
        assert self._genai_tokenizer is not None

        yield from generate_genai_chunks(
            genai,
            self._genai_model,
            self._genai_tokenizer,
            prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
        )

    # ── embeddings ──────────────────────────────────────────────────────────

    def embed(self, texts: list[str]) -> list[list[float]]:
        """Run the loaded InferenceSession against each text. Returns vectors.

        Raises :class:`OnnxRuntimeClientError` (``UnsupportedOperation``)
        when the loaded model is not an embedding session.
        """
        if self._info is None or not self._info.supports_embedding:
            raise OnnxRuntimeClientError(
                "UnsupportedOperation",
                "loaded ONNX model does not expose an embedding session",
            )
        if self._ort_session is None:
            raise OnnxRuntimeClientError(
                "BackendUnavailable", "InferenceSession was not loaded",
            )
        try:
            outputs = self._ort_session.run(None, {"input_text": list(texts)})
        except MemoryError as exc:
            raise OnnxRuntimeClientError(
                "OutOfMemory", f"ONNX Runtime memory exhausted: {exc}",
            ) from exc
        except Exception as exc:
            raise OnnxRuntimeClientError(
                "BackendCrashed", f"ONNX Runtime embed failed: {exc}",
            ) from exc

        if not outputs:
            return []
        first = outputs[0]
        try:
            return [list(map(float, row)) for row in first]
        except (TypeError, ValueError) as exc:
            raise OnnxRuntimeClientError(
                "ValidationError",
                f"ONNX session returned non-numeric embeddings: {exc}",
            ) from exc

    # ── readiness ───────────────────────────────────────────────────────────

    def is_ready(self) -> bool:
        """True when the client has loaded a usable backend."""
        if self._closed or self._info is None:
            return False
        return self._info.supports_generation or self._info.supports_embedding

    def info(self) -> LoadedModelInfo | None:
        """Return the post-load info struct, or None when unloaded."""
        return self._info

    # ── internals ───────────────────────────────────────────────────────────

    def _require_generation_ready(self) -> None:
        if self._info is None or not self._info.supports_generation:
            raise OnnxRuntimeClientError(
                "UnsupportedOperation",
                "loaded ONNX model does not support autoregressive generation",
            )

    def _load_genai(self, genai: Any) -> None:
        """Load a generative model via onnxruntime-genai."""
        try:
            model = genai.Model(self._model_path)
            tokenizer = genai.Tokenizer(model)
        except FileNotFoundError as exc:
            raise OnnxRuntimeClientError(
                "ModelNotFound", f"onnxruntime-genai could not open model: {exc}",
            ) from exc
        except MemoryError as exc:
            raise OnnxRuntimeClientError(
                "OutOfMemory", f"ONNX Runtime memory exhausted: {exc}",
            ) from exc
        except Exception as exc:
            raise OnnxRuntimeClientError(
                "BackendUnavailable",
                f"onnxruntime-genai refused to load model: {exc}",
            ) from exc
        self._genai_module = genai
        self._genai_model = model
        self._genai_tokenizer = tokenizer

    def _load_inference_session(self) -> None:
        """Load a plain InferenceSession for embedding / encoder models."""
        ort = self._ort_module
        assert ort is not None
        try:
            self._ort_session = ort.InferenceSession(
                self._model_path,
                providers=self._providers,
            )
        except FileNotFoundError as exc:
            raise OnnxRuntimeClientError(
                "ModelNotFound", f"ONNX Runtime could not open model: {exc}",
            ) from exc
        except MemoryError as exc:
            raise OnnxRuntimeClientError(
                "OutOfMemory", f"ONNX Runtime memory exhausted: {exc}",
            ) from exc
        except Exception as exc:
            raise OnnxRuntimeClientError(
                "BackendUnavailable",
                f"ONNX Runtime refused to load model: {exc}",
            ) from exc
