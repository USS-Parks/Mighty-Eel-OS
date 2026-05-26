"""ONNX Runtime client data shapes and local error type."""

from __future__ import annotations

from dataclasses import dataclass


class OnnxRuntimeClientError(RuntimeError):
    """Local client-side error. The adapter maps this to a typed MAI error."""

    def __init__(self, kind: str, detail: str) -> None:
        self.kind = kind
        self.detail = detail
        super().__init__(f"[{kind}] {detail}")


@dataclass
class OnnxStreamChunk:
    """One streaming step yielded by OnnxRuntimeClient.generate_stream."""

    text: str
    is_final: bool = False


@dataclass
class LoadedModelInfo:
    """What the client discovered about the loaded model."""

    supports_generation: bool
    supports_embedding: bool
    backend: str
    backend_version: str
    model_id: str
