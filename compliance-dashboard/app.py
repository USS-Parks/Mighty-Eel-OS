"""MAI Compliance Dashboard FastAPI app.

Single-process operator console. Renders seven pages:

* ``/``         — Overview: trust mode, module status, recent activity.
* ``/audit``    — Audit log viewer (searchable, paginated).
* ``/reports``  — Report list + generation form.
* ``/policy``   — Policy templates + per-module toggles.
* ``/alerts``   — Real-time alert feed (SSE in the browser).
* ``/monitoring`` — Runtime probes, scheduler, power, trust, metrics.
* ``/health``   — Compliance module health snapshot.

Every page calls the mai-api ``/v1/compliance/*`` and ``/v1/trust/*``
endpoints via the :mod:`mai` Python SDK — the dashboard never holds
its own state.

Run with::

    uvicorn compliance_dashboard.app:app --reload
"""

from __future__ import annotations

import html

from fastapi import Depends, FastAPI, HTTPException, Request, status
from fastapi.responses import HTMLResponse, JSONResponse, Response
from mai.client import MaiClient
from mai.errors import MaiError

from .alerts import severity_for
from .audit_viewer import AuditFilter, flatten_rows, normalise_decision, normalise_module
from .monitoring import MonitorPanel, collect_monitoring_snapshot
from .reports import (
    FORMAT_CHOICES,
    TEMPLATE_CHOICES,
    GenerateForm,
    summarise,
    template_label,
)
from .util import AUTH_TOKEN_HEADER, build_client, is_admin

app = FastAPI(title="MAI Compliance Dashboard", version="0.1.0")


def require_admin(request: Request) -> None:
    """Reject every request that doesn't carry the dashboard admin token."""
    if not is_admin(request.headers):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail=f"Missing or invalid {AUTH_TOKEN_HEADER} header",
        )


# Client factory is overridable in tests via `app.dependency_overrides`.
def get_client() -> MaiClient:
    return build_client()


ADMIN_DEP = Depends(require_admin)
CLIENT_DEP = Depends(get_client)


# --- Page chrome -----------------------------------------------------

_BASE_CSS = """
:root { color-scheme: light; --ink: #182026; --muted: #5c6770;
        --line: #d9dee3; --panel: #ffffff; --band: #f4f7f9;
        --ok-bg: #dcefe5; --ok-ink: #155b35; --warn-bg: #fff1c7;
        --warn-ink: #765200; --crit-bg: #f8d5d1; --crit-ink: #84251f; }
* { box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
       margin: 0; color: var(--ink); background: var(--band); }
body > h1, body > nav, body > .section, body > .monitor-grid {
       max-width: 1180px; margin-left: auto; margin-right: auto; }
body > h1 { margin-top: 1.5rem; margin-bottom: 0.75rem; font-size: 1.75rem; }
nav { display: flex; flex-wrap: wrap; gap: 0.5rem; padding-bottom: 1rem; }
nav a { color: var(--ink); text-decoration: none; border: 1px solid var(--line);
        background: var(--panel); padding: 0.45rem 0.7rem; border-radius: 6px; }
table { border-collapse: collapse; width: 100%; margin-top: 1rem; background: var(--panel); }
th, td { border: 1px solid var(--line); padding: 7px 10px; text-align: left;
         vertical-align: top; font-size: 0.9rem; }
th { background: #e9eef2; }
.badge { display: inline-block; padding: 2px 8px; border-radius: 4px;
         font-size: 0.8rem; font-weight: 700; }
.badge.ok { background: var(--ok-bg); color: var(--ok-ink); }
.badge.warn { background: var(--warn-bg); color: var(--warn-ink); }
.badge.crit { background: var(--crit-bg); color: var(--crit-ink); }
.section { margin-top: 1rem; background: var(--panel); border: 1px solid var(--line);
           border-radius: 8px; padding: 1rem; }
.monitor-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(310px, 1fr));
                gap: 0.9rem; margin-top: 1rem; margin-bottom: 2rem; }
.monitor-panel { background: var(--panel); border: 1px solid var(--line);
                 border-radius: 8px; padding: 1rem; min-height: 220px; }
.monitor-panel h2 { margin: 0 0 0.35rem; font-size: 1.05rem; }
.monitor-panel p { margin: 0.35rem 0 0.7rem; color: var(--muted); }
.monitor-panel dl { display: grid; grid-template-columns: minmax(110px, 42%) 1fr;
                    gap: 0.4rem 0.7rem; margin: 0; }
.monitor-panel dt { color: var(--muted); }
.monitor-panel dd { margin: 0; overflow-wrap: anywhere; }
form input, form select, form button { font-size: 1rem; padding: 4px 8px; }
form label { display: inline-block; min-width: 140px; margin-bottom: 0.4rem; }
"""


def _nav() -> str:
    return (
        '<nav><a href="/">Overview</a> <a href="/audit">Audit</a> '
        '<a href="/reports">Reports</a> <a href="/policy">Policy</a> '
        '<a href="/alerts">Alerts</a> <a href="/monitoring">Monitoring</a> '
        '<a href="/health">Health</a></nav>'
    )


def _page(title: str, body: str) -> str:
    title_safe = html.escape(title)
    return (
        f"<!doctype html><html><head><title>{title_safe} — MAI Compliance</title>"
        f"<style>{_BASE_CSS}</style></head>"
        f"<body><h1>{title_safe}</h1>{_nav()}{body}</body></html>"
    )


def _badge_class(severity: str) -> str:
    return {"info": "ok", "warning": "warn", "critical": "crit"}.get(severity, "ok")


def _state_badge(state: str) -> str:
    label = {"ok": "OK", "warn": "WATCH", "crit": "ACTION"}.get(state, state.upper())
    return f"<span class='badge {html.escape(state)}'>{html.escape(label)}</span>"


def _render_panel(panel: MonitorPanel) -> str:
    rows = "".join(
        f"<dt>{html.escape(label)}</dt><dd>{html.escape(value)}</dd>"
        for label, value in panel.rows
    )
    error = (
        f"<dt>error</dt><dd>{html.escape(panel.error)}</dd>"
        if panel.error
        else ""
    )
    return (
        "<section class='monitor-panel'>"
        f"<h2>{html.escape(panel.name)} {_state_badge(panel.state)}</h2>"
        f"<p>{html.escape(panel.summary)}</p>"
        f"<dl>{rows}{error}</dl></section>"
    )


# --- Overview --------------------------------------------------------

@app.get("/", response_class=HTMLResponse)
def overview(
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> HTMLResponse:
    try:
        trust = client.trust.status()
        compliance = client.compliance.get_status()
    except MaiError as exc:
        body = f"<p class='badge crit'>upstream error: {html.escape(str(exc))}</p>"
        return HTMLResponse(_page("Overview", body), status_code=502)

    rows = "".join(
        f"<tr><td>{html.escape(m.module)}</td>"
        f"<td>{'enabled' if m.enabled else 'disabled'}</td>"
        f"<td>{m.priority if m.priority is not None else ''}</td></tr>"
        for m in compliance.modules
    )
    body = (
        f"<section class='section'><h2>Trust Manifold</h2>"
        f"<p>Mode: <span class='badge ok'>{html.escape(trust.mode)}</span></p>"
        f"<p>Bundle: {html.escape(trust.bundle_version or '(none)')} "
        f"| Claims: {trust.claim_count}</p></section>"
        f"<section class='section'><h2>Compliance modules</h2>"
        f"<table><tr><th>Module</th><th>State</th><th>Priority</th></tr>"
        f"{rows}</table></section>"
        f"<section class='section'><h2>Audit chain</h2>"
        f"<p>Entries: {compliance.audit_integrity.entry_count} "
        f"| Last verify: {html.escape(compliance.audit_integrity.last_verify)}</p>"
        f"</section>"
    )
    return HTMLResponse(_page("Overview", body))


# --- Audit -----------------------------------------------------------

@app.get("/audit", response_class=HTMLResponse)
def audit_page(
    request: Request,
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> HTMLResponse:
    params = request.query_params
    filt = AuditFilter(
        module=normalise_module(params.get("module")),
        decision=normalise_decision(params.get("decision")),
        tenant=params.get("tenant") or None,
        limit=int(params.get("limit", "50") or "50"),
    )
    try:
        env = client.compliance.query_audit(**filt.sdk_kwargs())
    except MaiError as exc:
        return HTMLResponse(
            _page("Audit", f"<p class='badge crit'>{html.escape(str(exc))}</p>"),
            status_code=502,
        )
    rows = flatten_rows(env.rows)
    rendered = "".join(
        f"<tr><td>{r.entry_id}</td>"
        f"<td>{r.timestamp_unix_nanos}</td>"
        f"<td>{html.escape(r.decision)}</td>"
        f"<td>{html.escape(r.tenant)}</td>"
        f"<td>{html.escape(', '.join(r.modules_applied))}</td>"
        f"<td><span class='badge {'ok' if r.verification_badge == 'Verified' else 'warn'}'>"
        f"{html.escape(r.verification_badge)}</span></td></tr>"
        for r in rows
    )
    form = (
        "<form method='get' action='/audit'>"
        "<label>Module: <input name='module' value=''></label>"
        "<label>Decision: <input name='decision' value=''></label>"
        "<label>Tenant: <input name='tenant' value=''></label>"
        "<label>Limit: <input name='limit' value='50'></label>"
        "<button type='submit'>Search</button></form>"
    )
    body = (
        f"<section class='section'>{form}</section>"
        f"<section class='section'><h2>Results ({env.total})</h2>"
        f"<table><tr><th>id</th><th>timestamp</th><th>decision</th>"
        f"<th>tenant</th><th>modules</th><th>verify</th></tr>"
        f"{rendered}</table></section>"
    )
    return HTMLResponse(_page("Audit Log", body))


# --- Reports ---------------------------------------------------------

@app.get("/reports", response_class=HTMLResponse)
def reports_page(
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> HTMLResponse:
    try:
        env = client.compliance.list_reports()
    except MaiError as exc:
        return HTMLResponse(
            _page("Reports", f"<p class='badge crit'>{html.escape(str(exc))}</p>"),
            status_code=502,
        )
    summary = summarise(env.reports)
    options = "".join(
        f"<option value='{key}'>{html.escape(label)}</option>"
        for key, label in TEMPLATE_CHOICES
    )
    formats = "".join(
        f"<option value='{key}'>{html.escape(label)}</option>"
        for key, label in FORMAT_CHOICES
    )
    rows = "".join(
        f"<tr><td>{html.escape(r.id)}</td>"
        f"<td>{html.escape(template_label(r.report_type))}</td>"
        f"<td><span class='badge {'ok' if r.status == 'complete' else 'warn'}'>"
        f"{html.escape(r.status)}</span></td>"
        f"<td>{html.escape(r.output_format)}</td>"
        f"<td>{html.escape(r.tenant or '')}</td>"
        f"<td><a href='/reports/{html.escape(r.id)}/download'>download</a></td></tr>"
        for r in env.reports
    )
    body = (
        f"<section class='section'><h2>Summary</h2>"  # nosec B608 — HTML template, every var passes through html.escape; not a SQL statement
        f"<p>Total: {summary.total} | Complete: {summary.complete} "
        f"| Pending: {summary.pending} | Failed: {summary.failed}</p></section>"
        f"<section class='section'><h2>Generate report</h2>"
        f"<form method='post' action='/reports/generate'>"
        f"<label>Type: <select name='report_type'>{options}</select></label>"
        f"<label>Format: <select name='format'>{formats}</select></label>"
        f"<label>From (ns): <input name='from_unix_nanos' value='0'></label>"
        f"<label>To (ns): <input name='to_unix_nanos' value='1000000000000'></label>"
        f"<label>Tenant: <input name='tenant' value=''></label>"
        f"<button type='submit'>Generate</button></form></section>"
        f"<section class='section'><h2>Records</h2>"
        f"<table><tr><th>id</th><th>type</th><th>status</th><th>format</th>"
        f"<th>tenant</th><th></th></tr>{rows}</table></section>"
    )
    return HTMLResponse(_page("Reports", body))


@app.post("/reports/generate")
async def reports_generate(
    request: Request,
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> Response:
    form = await request.form()
    try:
        gf = GenerateForm(
            report_type=str(form.get("report_type", "system_activity")),
            from_unix_nanos=int(str(form.get("from_unix_nanos", "0")) or "0"),
            to_unix_nanos=int(str(form.get("to_unix_nanos", "0")) or "0"),
            format=str(form.get("format", "json")),
            tenant=str(form.get("tenant") or "") or None,
        )
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    errors = gf.validate()
    if errors:
        raise HTTPException(status_code=400, detail="; ".join(errors))
    record = client.compliance.generate_report(**gf.sdk_kwargs())
    return JSONResponse(record.model_dump())


@app.get("/reports/{report_id}/download")
def reports_download(
    report_id: str,
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> Response:
    record = client.compliance.get_report(report_id)
    body = client.compliance.download_report(report_id)
    media_type = {
        "json": "application/json",
        "html": "text/html",
        "csv": "text/csv",
        "text": "text/plain",
    }.get(record.output_format, "application/octet-stream")
    return Response(content=body, media_type=media_type)


# --- Policy ----------------------------------------------------------

@app.get("/policy", response_class=HTMLResponse)
def policy_page(
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> HTMLResponse:
    try:
        modules = client.compliance.get_policies()
    except MaiError as exc:
        return HTMLResponse(
            _page("Policy", f"<p class='badge crit'>{html.escape(str(exc))}</p>"),
            status_code=502,
        )
    rows = "".join(
        f"<tr><td>{html.escape(str(m.get('module', '?')))}</td>"
        f"<td>{'on' if m.get('enabled') else 'off'}</td>"
        f"<td>{m.get('priority') if m.get('priority') is not None else ''}</td>"
        f"<td><form method='post' action='/policy/{html.escape(str(m.get('module', '')))}/toggle'>"
        f"<input type='hidden' name='enabled' value='{'false' if m.get('enabled') else 'true'}'>"
        f"<button type='submit'>Flip</button></form></td></tr>"
        for m in modules
    )
    body = (
        "<section class='section'><h2>Templates</h2>"
        "<form method='post' action='/policy/template'>"
        "<label>Template: "
        "<select name='template'>"
        "<option value='standard'>Standard</option>"
        "<option value='healthcare'>Healthcare</option>"
        "<option value='defense'>Defense</option>"
        "<option value='tribal_government'>Tribal Government</option>"
        "</select></label>"
        "<button type='submit'>Apply</button></form></section>"
        "<section class='section'><h2>Modules</h2>"
        "<table><tr><th>module</th><th>state</th><th>priority</th><th></th></tr>"
        f"{rows}</table></section>"
    )
    return HTMLResponse(_page("Policy", body))


@app.post("/policy/{module}/toggle")
async def policy_toggle(
    module: str,
    request: Request,
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> JSONResponse:
    form = await request.form()
    enabled = str(form.get("enabled", "false")).lower() == "true"
    result = client.compliance.update_policy(module, enabled=enabled)
    return JSONResponse(result)


@app.post("/policy/template")
async def policy_template(
    request: Request,
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> JSONResponse:
    form = await request.form()
    template = str(form.get("template", "standard"))
    return JSONResponse(client.compliance.apply_template(template))


# --- Alerts ----------------------------------------------------------

@app.get("/alerts", response_class=HTMLResponse)
def alerts_page(_: None = ADMIN_DEP) -> HTMLResponse:
    body = (
        "<section class='section'>"
        "<p>Live event stream. Subscribe via <code>EventSource</code> to "
        "<code>/v1/compliance/feed</code> on the mai-api server.</p>"
        "<table><tr><th>kind</th><th>severity</th></tr>"
        f"<tr><td>decision_made</td><td><span class='badge ok'>"
        f"{severity_for('decision_made')}</span></td></tr>"
        f"<tr><td>policy_changed</td><td><span class='badge ok'>"
        f"{severity_for('policy_changed')}</span></td></tr>"
        f"<tr><td>module_state_changed</td><td><span class='badge warn'>"
        f"{severity_for('module_state_changed')}</span></td></tr>"
        f"<tr><td>violation_detected</td><td><span class='badge crit'>"
        f"{severity_for('violation_detected')}</span></td></tr>"
        "</table></section>"
    )
    return HTMLResponse(_page("Alerts", body))


# --- Monitoring ------------------------------------------------------

@app.get("/monitoring", response_class=HTMLResponse)
def monitoring_page(
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> HTMLResponse:
    panels = collect_monitoring_snapshot(client)
    body = "<main class='monitor-grid'>" + "".join(_render_panel(p) for p in panels) + "</main>"
    status_code = 200 if all(p.state != "crit" for p in panels) else 502
    return HTMLResponse(_page("Monitoring", body), status_code=status_code)


# --- Health ----------------------------------------------------------

@app.get("/health", response_class=JSONResponse)
def health(
    _: None = ADMIN_DEP,
    client: MaiClient = CLIENT_DEP,
) -> JSONResponse:
    try:
        status_payload = client.compliance.get_status()
    except MaiError as exc:
        return JSONResponse({"healthy": False, "error": str(exc)}, status_code=502)
    integrity = status_payload.audit_integrity
    return JSONResponse({
        "healthy": integrity.last_verify in {"verified", "unknown"},
        "modules": [m.model_dump() for m in status_payload.modules],
        "audit_integrity": integrity.model_dump(),
        "subscribers": status_payload.subscribers,
        "reload_count": status_payload.reload_count,
    })


__all__ = ["app", "get_client", "require_admin"]


def _badge_class_proxy(severity: str) -> str:  # pragma: no cover — re-export
    return _badge_class(severity)
