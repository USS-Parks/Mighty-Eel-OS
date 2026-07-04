"""Operator/Admin — read-only dashboard for a local MAI instance.

Pulls one panel per area (models, scheduler, adapters, power, audit,
trust, airgap) and renders to stdout as a plain-text snapshot. JSON
output via ``--json``. Designed for cron / on-call use, not pretty
TUI rendering.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from mai import MaiClient, MaiClientConfig, MaiError

DEFAULT_CONFIG = Path(__file__).with_name("config.toml")


# ---------------------------------------------------------------------------
# Snapshot shape
# ---------------------------------------------------------------------------

@dataclass
class Panel:
    name: str
    ok: bool
    summary: str
    detail: dict[str, Any] = field(default_factory=dict)
    error: str | None = None


@dataclass
class DashboardSnapshot:
    panels: list[Panel] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {"panels": [
            {"name": p.name, "ok": p.ok, "summary": p.summary,
             "detail": p.detail, "error": p.error}
            for p in self.panels
        ]}


# ---------------------------------------------------------------------------
# Per-panel collectors
# ---------------------------------------------------------------------------

def _safe(name: str, fn) -> Panel:  # type: ignore[no-untyped-def]
    """Run a collector, catching MaiError into a Panel(ok=False)."""
    try:
        return fn()
    except MaiError as e:
        return Panel(name=name, ok=False, summary="ERROR",
                     error=f"{type(e).__name__}: {e}")


def panel_models(client: MaiClient) -> Panel:
    return _safe("models", lambda: _do_models(client))


def _do_models(client: MaiClient) -> Panel:
    models = client.models.list()
    loaded = [m for m in models
              if m.status.value in ("loaded", "active")]
    return Panel(
        name="models", ok=True,
        summary=f"{len(loaded)}/{len(models)} loaded",
        detail={"models": [
            {"id": m.id, "status": m.status.value,
             "size_bytes": m.size_bytes,
             "format": m.format.value}
            for m in models
        ]},
    )


def panel_scheduler(client: MaiClient) -> Panel:
    return _safe("scheduler", lambda: _do_scheduler(client))


def _do_scheduler(client: MaiClient) -> Panel:
    m = client.scheduler.metrics()
    return Panel(
        name="scheduler", ok=True,
        summary=f"queue={m.queue_depth} active={m.active_requests} "
                f"p95={m.p95_wait_ms:.1f}ms",
        detail={
            "queue_depth": m.queue_depth,
            "active_requests": m.active_requests,
            "scheduled_total": m.scheduled_total,
            "rejected_total": m.rejected_total,
            "avg_wait_ms": m.avg_wait_ms,
            "p95_wait_ms": m.p95_wait_ms,
            "instances": m.instances,
        },
    )


def panel_adapters(client: MaiClient) -> Panel:
    return _safe("adapters", lambda: _do_adapters(client))


def _do_adapters(client: MaiClient) -> Panel:
    data = client.admin.adapters()
    return Panel(
        name="adapters", ok=True,
        summary=f"{len(data) if isinstance(data, list) else 'n/a'} adapter(s)",
        detail={"raw": data},
    )


def panel_power(client: MaiClient) -> Panel:
    return _safe("power", lambda: _do_power(client))


def _do_power(client: MaiClient) -> Panel:
    p = client.power.get_state()
    return Panel(
        name="power", ok=True,
        summary=f"state={p.state.value} "
                f"~{p.estimated_power_watts:.0f}W",
        detail={
            "state": p.state.value,
            "estimated_power_watts": p.estimated_power_watts,
            "auto_demotion_enabled": p.auto_demotion.enabled,
            "promotion_available": p.promotion_available,
            "promotion_latency_target_ms": p.promotion_latency_target_ms,
        },
    )


def panel_airgap(client: MaiClient) -> Panel:
    return _safe("airgap", lambda: _do_airgap(client))


def _do_airgap(client: MaiClient) -> Panel:
    s = client.system.airgap()
    return Panel(
        name="airgap", ok=s.air_gap_verified,
        summary=f"enabled={s.air_gap_enabled} verified={s.air_gap_verified} "
                f"net={s.network_state.value}",
        detail={
            "air_gap_enabled": s.air_gap_enabled,
            "air_gap_verified": s.air_gap_verified,
            "network_state": s.network_state.value,
            "last_check_unix": s.last_check_unix,
            "violations_24h": s.violations_24h,
        },
    )


def panel_audit(client: MaiClient, limit: int) -> Panel:
    return _safe("audit", lambda: _do_audit(client, limit))


def _do_audit(client: MaiClient, limit: int) -> Panel:
    log = client.admin.audit_log(limit=limit)
    return Panel(
        name="audit", ok=True,
        summary=f"{log.total_entries} total, showing {len(log.entries)}",
        detail={"entries": [{
            "timestamp": e.timestamp.isoformat(),
            "endpoint": e.endpoint,
            "method": e.method,
            "status_code": e.status_code,
            "model": e.model,
            "tokens_in": e.tokens_in,
            "tokens_out": e.tokens_out,
        } for e in log.entries]},
    )


def panel_trust(client: MaiClient) -> Panel:
    return _safe("trust", lambda: _do_trust(client))


def _do_trust(client: MaiClient) -> Panel:
    """live: read /v1/trust/status for the consolidated dashboard view."""
    status = client.trust.status()
    return Panel(
        name="trust", ok=True,
        summary=f"mode={status.mode} bundle={status.bundle_version or '-'}",
        detail={
            "mode": status.mode,
            "bundle_version": status.bundle_version,
            "last_refresh_secs": status.last_refresh_secs,
            "age_secs": status.age_secs,
            "claim_count": status.claim_count,
            "offline_backlog": status.offline_backlog,
            "airgap": status.airgap,
        },
    )


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------

def render_text(snap: DashboardSnapshot) -> str:
    lines = ["=== MAI Operator Dashboard ==="]
    for p in snap.panels:
        marker = "OK " if p.ok else "ERR"
        lines.append(f"[{marker}] {p.name:<10} {p.summary}")
        if p.error:
            lines.append(f"      ! {p.error}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Hook + entry point
# ---------------------------------------------------------------------------

def load_app_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    if not path.exists():
        return {}
    with path.open("rb") as fh:
        return tomllib.load(fh)


def _make_client(sdk_config: MaiClientConfig) -> MaiClient:
    return MaiClient(sdk_config)


def collect(client: MaiClient, display_cfg: dict[str, Any]) -> DashboardSnapshot:
    snap = DashboardSnapshot()
    if display_cfg.get("models", True):
        snap.panels.append(panel_models(client))
    if display_cfg.get("scheduler", True):
        snap.panels.append(panel_scheduler(client))
    if display_cfg.get("adapters", True):
        snap.panels.append(panel_adapters(client))
    if display_cfg.get("power", True):
        snap.panels.append(panel_power(client))
    if display_cfg.get("airgap", True):
        snap.panels.append(panel_airgap(client))
    if display_cfg.get("audit", True):
        snap.panels.append(panel_audit(client, int(display_cfg.get("audit_limit", 10))))
    if display_cfg.get("trust", True):
        snap.panels.append(panel_trust(client))
    return snap


def run(*, config_path: Path = DEFAULT_CONFIG,
        as_json: bool = False) -> int:
    cfg = load_app_config(config_path)
    display_cfg = cfg.get("display", {})
    client_overrides = cfg.get("client", {})
    sdk_config = MaiClientConfig.load(**client_overrides)

    with _make_client(sdk_config) as client:
        if not client.health_check():
            print("MAI server unreachable", file=sys.stderr)
            return 1
        snap = collect(client, display_cfg)

    if as_json:
        print(json.dumps(snap.to_dict(), indent=2))
    else:
        print(render_text(snap))

    # Non-zero exit if any *core* panel failed (audit/trust stubs don't count).
    core = {"models", "scheduler", "power", "airgap"}
    if any((not p.ok) and p.name in core for p in snap.panels):
        return 5
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="operator",
        description="Read-only MAI operator/admin dashboard.",
    )
    parser.add_argument("--config", default=str(DEFAULT_CONFIG))
    parser.add_argument("--json", action="store_true",
                        help="emit the snapshot as JSON")
    args = parser.parse_args(argv)
    return run(config_path=Path(args.config), as_json=args.json)


if __name__ == "__main__":
    sys.exit(main())
