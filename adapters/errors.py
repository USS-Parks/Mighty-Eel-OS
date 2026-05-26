"""Typed adapter error taxonomy shared by all backend adapters."""

from __future__ import annotations

from typing import Any


class AdapterError(Exception):
    """Base exception for adapter failures.

    All variants carry structured data that maps 1:1 to Rust AdapterError
    enum variants across the FFI boundary.
    """

    def __init__(self, code: str, detail: str | None = None, **kwargs: Any):
        self.code = code
        self.detail = detail
        self.data = kwargs
        super().__init__(f"[{code}] {detail or ''}")


class AdapterTimeoutError(AdapterError):
    """Backend exceeded response deadline."""

    def __init__(self, timeout_ms: int | str = 0):
        detail = (
            str(timeout_ms) if isinstance(timeout_ms, str) else f"Timed out after {timeout_ms}ms"
        )
        super().__init__(code="Timeout", detail=detail, timeout_ms=timeout_ms)


class OutOfMemoryError(AdapterError):
    """VRAM/host memory exhausted."""

    def __init__(self):
        super().__init__(code="OutOfMemory", detail="Backend out of memory")


class ModelNotFoundError(AdapterError):
    """Requested model not loaded or available."""

    def __init__(self, model: str):
        super().__init__(code="ModelNotFound", detail=f"Model '{model}' not found", model=model)


class BackendCrashedError(AdapterError):
    """Adapter process terminated unexpectedly."""

    def __init__(self, detail: str | None = None):
        super().__init__(code="BackendCrashed", detail=detail or "Backend process crashed")


class BackendUnavailableError(AdapterError):
    """Port or socket not listening."""

    def __init__(self, detail: str | None = None):
        super().__init__(code="BackendUnavailable", detail=detail or "Backend service unavailable")


class ContextExceededError(AdapterError):
    """Prompt exceeds max_context_window."""

    def __init__(self, max_context: int):
        super().__init__(
            code="ContextExceeded", detail=f"Exceeds {max_context} tokens", max_context=max_context,
        )


class RateLimitedError(AdapterError):
    """Backend throttling active."""

    def __init__(self):
        super().__init__(code="RateLimited", detail="Backend rate limited")


class HardwareFaultError(AdapterError):
    """HIL-reported GPU/memristor failure."""

    def __init__(self, detail: str):
        super().__init__(code="HardwareFault", detail=detail)


class ValidationError(AdapterError):
    """Config/schema mismatch."""

    def __init__(self, reason: str):
        super().__init__(code="ValidationError", detail=reason, reason=reason)


class UnsupportedOperationError(AdapterError):
    """Operation not supported by this backend."""

    def __init__(self, operation: str):
        super().__init__(
            code="UnsupportedOperation", detail=f"Not supported: {operation}", operation=operation,
        )
