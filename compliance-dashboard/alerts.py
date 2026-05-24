"""Real-time alert feed helper.

Wraps the mai-api ``GET /v1/compliance/feed`` SSE stream into an
async iterator the dashboard's `/alerts` route can render. Alerts
are surfaced as :class:`Alert` records with a normalised severity
and a short human-readable headline.

The mai-api server publishes four event kinds:

* ``decision_made`` — informational; rendered as severity ``info``.
* ``policy_changed`` — informational; tracks operator-driven config swaps.
* ``module_state_changed`` — warning; tracks runtime enable/disable.
* ``violation_detected`` — critical; non-allow decision detected.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

import httpx

from .util import api_base_url, auth_headers

# Severity tiers in increasing order of urgency.
SEVERITY_INFO = "info"
SEVERITY_WARNING = "warning"
SEVERITY_CRITICAL = "critical"

_KIND_TO_SEVERITY = {
    "decision_made": SEVERITY_INFO,
    "policy_changed": SEVERITY_INFO,
    "module_state_changed": SEVERITY_WARNING,
    "violation_detected": SEVERITY_CRITICAL,
}


@dataclass
class Alert:
    """Normalised alert row for the dashboard table."""

    kind: str
    severity: str
    timestamp_unix_ms: int
    headline: str
    payload: dict[str, Any] = field(default_factory=dict)


def severity_for(kind: str) -> str:
    """Map a feed event kind to a dashboard severity tier."""
    return _KIND_TO_SEVERITY.get(kind, SEVERITY_INFO)


def headline_for(event: dict[str, Any]) -> str:
    """Render a one-line human summary of a feed event."""
    kind = event.get("kind", "unknown")
    if kind == "decision_made":
        decision = event.get("decision", {})
        return (
            f"decision tenant={event.get('tenant_id', '?')} "
            f"allowed={decision.get('allowed', '?')}"
        )
    if kind == "policy_changed":
        return f"policy changed: {event.get('summary', '?')}"
    if kind == "module_state_changed":
        return f"module {event.get('module', '?')} -> enabled={event.get('enabled', '?')}"
    if kind == "violation_detected":
        return f"violation tenant={event.get('tenant_id', '?')}"
    return kind


def alert_from_event(event: dict[str, Any]) -> Alert:
    """Wrap a raw feed event dict in an :class:`Alert`."""
    kind = str(event.get("kind", "unknown"))
    return Alert(
        kind=kind,
        severity=severity_for(kind),
        timestamp_unix_ms=int(event.get("timestamp_unix_ms", 0)),
        headline=headline_for(event),
        payload=event,
    )


async def stream_alerts(
    base_url: str | None = None,
    token: str | None = None,
    *,
    max_events: int | None = None,
) -> list[Alert]:
    """Drain alerts off the SSE feed and return as a list.

    Used by the dashboard's polling fallback (the live SSE pane uses
    the browser's ``EventSource``; this helper drives synchronous
    tests + the "snapshot recent alerts" view).
    """
    import json

    url = f"{base_url or api_base_url()}/compliance/feed"
    out: list[Alert] = []
    async with httpx.AsyncClient(timeout=5.0) as http:
        async with http.stream("GET", url, headers=auth_headers(token)) as resp:
            resp.raise_for_status()
            async for line in resp.aiter_lines():
                if not line or line.startswith(":"):
                    continue
                if line.startswith("data:"):
                    raw = line[5:].strip()
                    if not raw:
                        continue
                    try:
                        event = json.loads(raw)
                    except json.JSONDecodeError:
                        continue
                    out.append(alert_from_event(event))
                    if max_events is not None and len(out) >= max_events:
                        break
    return out


__all__ = [
    "SEVERITY_CRITICAL",
    "SEVERITY_INFO",
    "SEVERITY_WARNING",
    "Alert",
    "alert_from_event",
    "headline_for",
    "severity_for",
    "stream_alerts",
]
