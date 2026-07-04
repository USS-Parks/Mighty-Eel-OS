"""MAI SDK exception hierarchy.

Maps server-side error responses (ErrorResponse) and transport-level
httpx failures into a stable Python exception tree the application
code can catch by class.

Hierarchy::

    MaiError                        # base
        BadRequestError             # 400
        AuthenticationError         # 401
        PermissionError             # 403
        NotFoundError               # 404
        RateLimitError              # 429 (carries retry_after)
        ServerError                 # 5xx
        ConnectionError             # network failure (no response)
        TimeoutError                # request timed out
        AirGapViolationError        # server reports air_gap_violation
        PowerStateUnavailableError  # server reports power_state_unavailable
        ClaimExpiredError           # trust claim expired
        TrustCacheStaleError        # local trust cache stale/expired

The legacy ``mai.types.MaiError`` re-imports the new ``MaiError`` so
existing code keeps working.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mai.types import ErrorResponse

_RETRYABLE_TYPES: frozenset[str] = frozenset({
    "rate_limited",
    "request_failed",
    "overloaded",
    "power_state_unavailable",
    "timeout",
})


class MaiError(Exception):
    """Base SDK error.

    Carries the original ``ErrorResponse`` when one is available
    (server-side error) and an HTTP status code when known.
    """

    status_code: int | None = None
    code: str | None = None
    error_type: str | None = None
    retry_after: int | None = None
    request_id: str | None = None

    def __init__(
        self,
        message: str,
        *,
        response: ErrorResponse | None = None,
        status_code: int | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.response = response
        if response is not None:
            self.code = response.error.code
            self.error_type = response.error.type.value
            self.retry_after = response.error.retry_after_seconds
            self.request_id = (
                str(response.error.request_id) if response.error.request_id else None
            )
        if status_code is not None:
            self.status_code = status_code

    @property
    def is_retryable(self) -> bool:
        """Whether the client should retry this request.

        Retryable: 429 (rate limit), 500/502/503/504 (transient server),
        plus any error_type tagged by the server as transient
        (rate_limited, request_failed, overloaded, power_state_unavailable,
        timeout). Explicitly non-retryable: 400, 401, 403, 404, 501, 505.
        """
        if self.error_type is not None and self.error_type in _RETRYABLE_TYPES:
            return True
        return self.status_code in (429, 500, 502, 503, 504)


class BadRequestError(MaiError):
    """HTTP 400 — malformed request, validation failed."""

    status_code = 400


class AuthenticationError(MaiError):
    """HTTP 401 — missing or invalid credentials."""

    status_code = 401


class PermissionError(MaiError):  # noqa: A001 — intentional shadow of builtin in SDK namespace
    """HTTP 403 — credentials valid but action not permitted."""

    status_code = 403


class NotFoundError(MaiError):
    """HTTP 404 — resource does not exist."""

    status_code = 404


class RateLimitError(MaiError):
    """HTTP 429 — rate limit exceeded. ``retry_after`` is set."""

    status_code = 429


class ServerError(MaiError):
    """HTTP 5xx — server-side failure."""

    status_code = 500


class ConnectionError(MaiError):  # noqa: A001
    """Network-level failure: cannot reach the server."""


class TimeoutError(MaiError):  # noqa: A001
    """Request exceeded its timeout."""


class AirGapViolationError(MaiError):
    """Server reported an air-gap policy violation."""

    status_code = 403


class PowerStateUnavailableError(MaiError):
    """Server is in a power state that cannot serve this request."""

    status_code = 503


class ClaimExpiredError(AuthenticationError):
    """Trust Manifold claim has expired.

    Subclass of ``AuthenticationError`` so existing 401 handlers catch it.
    """


class TrustCacheStaleError(MaiError):
    """Local trust cache is stale or expired.

    Raised by the SDK when the local trust cache cannot validate the
    server's claim and the connectivity state is degraded/stale/expired.
    """


def from_response(
    response: ErrorResponse,
    status_code: int,
) -> MaiError:
    """Map an HTTP status + ErrorResponse to the right exception class."""
    error_type = response.error.type.value
    message = response.error.message

    if error_type == "authentication_failed" or status_code == 401:
        if response.error.code in {"MAI-A101", "MAI-A102"}:
            return ClaimExpiredError(message, response=response, status_code=status_code)
        return AuthenticationError(message, response=response, status_code=status_code)
    if error_type == "permission_denied" or status_code == 403:
        if error_type == "air_gap_violation":
            return AirGapViolationError(message, response=response, status_code=status_code)
        return PermissionError(message, response=response, status_code=status_code)
    if status_code == 404:
        return NotFoundError(message, response=response, status_code=status_code)
    if status_code == 429 or error_type == "rate_limited":
        return RateLimitError(message, response=response, status_code=status_code)
    if error_type == "power_state_unavailable":
        return PowerStateUnavailableError(
            message, response=response, status_code=status_code,
        )
    if error_type == "timeout":
        return TimeoutError(message, response=response, status_code=status_code)
    if 500 <= status_code < 600:
        return ServerError(message, response=response, status_code=status_code)
    if status_code == 400 or error_type == "invalid_request":
        return BadRequestError(message, response=response, status_code=status_code)
    return MaiError(message, response=response, status_code=status_code)


def from_transport(exc: Exception) -> MaiError:
    """Map an httpx transport-level exception to an SDK exception."""
    import httpx

    if isinstance(exc, httpx.TimeoutException):
        return TimeoutError(f"request timed out: {exc}")
    if isinstance(exc, httpx.TransportError):
        return ConnectionError(f"network error: {exc}")
    return MaiError(f"transport error: {exc}")


__all__ = [
    "AirGapViolationError",
    "AuthenticationError",
    "BadRequestError",
    "ClaimExpiredError",
    "ConnectionError",
    "MaiError",
    "NotFoundError",
    "PermissionError",
    "PowerStateUnavailableError",
    "RateLimitError",
    "ServerError",
    "TimeoutError",
    "TrustCacheStaleError",
    "from_response",
    "from_transport",
]
