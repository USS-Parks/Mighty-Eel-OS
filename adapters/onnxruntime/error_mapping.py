"""Map ONNX Runtime client errors to MAI adapter errors."""

from __future__ import annotations

from adapters.base import (
    AdapterError,
    BackendCrashedError,
    BackendUnavailableError,
    ModelNotFoundError,
    OutOfMemoryError,
    UnsupportedOperationError,
    ValidationError,
)


def raise_typed(kind: str, detail: str, *, model_id: str = "") -> None:
    """Translate a client-error kind into the matching MAI typed error."""
    if kind == "ValidationError":
        raise ValidationError(detail)
    if kind == "BackendUnavailable":
        raise BackendUnavailableError(detail)
    if kind == "ModelNotFound":
        raise ModelNotFoundError(model_id or detail)
    if kind == "OutOfMemory":
        raise OutOfMemoryError()
    if kind == "UnsupportedOperation":
        raise UnsupportedOperationError(detail)
    if kind == "BackendCrashed":
        raise BackendCrashedError(detail)
    raise AdapterError(code=kind or "InternalError", detail=detail)
