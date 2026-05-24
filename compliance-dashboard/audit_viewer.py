"""Audit log viewer helpers.

Wraps the SDK's :meth:`mai.client.MaiClient.compliance.query_audit`
with dashboard-friendly projections: filter normalisation, row
flattening, pagination, and the chain-verification badge logic.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from mai.types import AuditRow

DEFAULT_PAGE_SIZE = 50
MAX_PAGE_SIZE = 500

# Valid filter values exposed through the dashboard query string.
KNOWN_MODULES = {"hipaa", "itar", "ear", "ocap"}
KNOWN_DECISIONS = {"allow", "local_only", "quarantine", "deny"}


@dataclass
class AuditFilter:
    """Filter shape carried from the dashboard's search form."""

    from_unix_nanos: int | None = None
    to_unix_nanos: int | None = None
    module: str | None = None
    decision: str | None = None
    tenant: str | None = None
    limit: int = DEFAULT_PAGE_SIZE

    def sdk_kwargs(self) -> dict[str, Any]:
        """Project this filter to the SDK ``query_audit`` keyword args."""
        out: dict[str, Any] = {"limit": clamp_limit(self.limit)}
        if self.from_unix_nanos is not None:
            out["from_unix_nanos"] = self.from_unix_nanos
        if self.to_unix_nanos is not None:
            out["to_unix_nanos"] = self.to_unix_nanos
        if self.module:
            out["module"] = self.module
        if self.decision:
            out["decision"] = self.decision
        if self.tenant:
            out["tenant"] = self.tenant
        return out


@dataclass
class AuditDisplayRow:
    """Flattened row optimised for HTML table rendering."""

    entry_id: int
    timestamp_unix_nanos: int
    decision: str
    tenant: str
    modules_applied: list[str]
    verification_badge: str
    raw: dict[str, Any]


def clamp_limit(limit: int | None) -> int:
    """Coerce a caller-supplied page size into the dashboard's bounds."""
    if not limit or limit <= 0:
        return DEFAULT_PAGE_SIZE
    return min(int(limit), MAX_PAGE_SIZE)


def normalise_module(raw: str | None) -> str | None:
    """Return a lower-cased module id when it's one the server accepts."""
    if not raw:
        return None
    candidate = raw.strip().lower()
    return candidate if candidate in KNOWN_MODULES else None


def normalise_decision(raw: str | None) -> str | None:
    """Return a lower-cased decision string when it's accepted."""
    if not raw:
        return None
    candidate = raw.strip().lower()
    return candidate if candidate in KNOWN_DECISIONS else None


def verification_badge(status: str) -> str:
    """Map a verification status to a dashboard badge label."""
    return {
        "verified": "Verified",
        "tampered": "TAMPERED",
        "unknown": "Pending",
    }.get(status, "Pending")


def flatten_row(row: AuditRow) -> AuditDisplayRow:
    """Flatten an SDK :class:`AuditRow` into a display projection."""
    entry = dict(row.entry)
    correlation = entry.get("correlation", {}) or {}
    return AuditDisplayRow(
        entry_id=int(entry.get("id", 0)),
        timestamp_unix_nanos=int(entry.get("timestamp_unix_nanos", 0)),
        decision=str(entry.get("decision", "?")),
        tenant=str(correlation.get("tenant", "")),
        modules_applied=list(entry.get("modules_applied", []) or []),
        verification_badge=verification_badge(row.status),
        raw=entry,
    )


def flatten_rows(rows: list[AuditRow]) -> list[AuditDisplayRow]:
    """Flatten a page of rows for the audit table."""
    return [flatten_row(r) for r in rows]


__all__ = [
    "DEFAULT_PAGE_SIZE",
    "KNOWN_DECISIONS",
    "KNOWN_MODULES",
    "MAX_PAGE_SIZE",
    "AuditDisplayRow",
    "AuditFilter",
    "clamp_limit",
    "flatten_row",
    "flatten_rows",
    "normalise_decision",
    "normalise_module",
    "verification_badge",
]
