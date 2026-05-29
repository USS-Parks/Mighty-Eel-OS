# Observability

What the appliance exports, how to consume it, and which signals
should page someone. For routine operator cadence see
[OPERATIONS.md](OPERATIONS.md); for the alert -> runbook map
see [runbooks/README.md](runbooks/README.md).

## Surfaces

| Surface | Scheme | Auth | Purpose |
|---|---|---|---|
| `/v1/health/live` | HTTP GET | none | Liveness; the process is alive |
| `/v1/health/ready` | HTTP GET | admin token | Readiness; deps wired, validator passed |
| `/v1/health/adapters` | HTTP GET | admin token | Per-adapter state + restart counter |
| `/v1/health/hardware` | HTTP GET | admin token | GPU + topology snapshot |
| `/v1/metrics` | HTTP GET | admin token | Prometheus scrape |
| `/v1/system/connectivity` | HTTP GET | admin token | Air-gap posture, recent egress attempts |
| `/v1/system/trust/status` | HTTP GET | admin token | Anchor + bundle state |
| `/v1/system/production-readiness` | HTTP GET | admin token | Live `mai-ship-validate` shape |
| structured logs | stderr -> journal | n/a | Per-event audit-adjacent log |
| audit WAL | local file | n/a (signed) | Tamper-evident chain |

## Health endpoint shape

`/v1/health/live` returns 200 with `{ "status": "live" }` once
the process is up. It does **not** consult the validator; a
live-but-not-ready daemon is a normal startup state.

`/v1/health/ready` returns 200 only when every `PROD-*` check
passes. The same evaluator runs inside `mai-ship-validate`. A
failing readiness probe carries the failing check IDs in its
response body — your reverse proxy should treat any non-200 as
unhealthy.

## Prometheus metrics

Exposed on `/v1/metrics`. Metric families and dashboards live
under [`compliance-dashboard/dashboards/`](../compliance-dashboard/dashboards/).
Key families:

- `mai_request_duration_seconds{route,profile,status}` —
  histogram of API request latency.
- `mai_queue_depth{role}` — admission queue depth by role.
- `mai_kv_cache_bytes_used{instance}` — KV cache utilization.
- `mai_adapter_restarts_total{adapter}` — adapter restart
  counter; non-zero is signal.
- `mai_audit_chain_last_seq` — WAL tail; should advance
  monotonically.
- `mai_audit_verify_duration_seconds` — full-chain verify cost;
  baseline to detect chain growth anomalies.
- `mai_policy_decision_total{module,outcome}` — Lamprey policy
  evaluation outcomes; outcome ∈ `{allow,redact,block,error}`.
- `mai_trust_bundle_seconds_until_expiry` — bundle expiry
  countdown; alert below 7 days.

The Prometheus scrape target should pull every 15 seconds and
retain at least 14 days locally. Anything shorter loses the
weekly-cycle context the operator needs for capacity decisions.

## Alert rule set

Reference rules live in
[`compliance-dashboard/alerts/`](../compliance-dashboard/alerts/).
Each rule maps to exactly one runbook:

| Alert | Trigger | Runbook |
|---|---|---|
| `mai_audit_chain_break` | `mai-admin audit verify` exited non-zero | [12-audit-wal-tamper](runbooks/12-audit-wal-tamper.md) |
| `mai_trust_bundle_expiring` | `bundle_seconds_until_expiry` < 7 days | [04-install-policy-bundle](runbooks/04-install-policy-bundle.md) |
| `mai_trust_bundle_expired` | bundle is past `not_after` | [11-trust-bundle-expired](runbooks/11-trust-bundle-expired.md) |
| `mai_trust_anchor_missing` | anchors count drops to 0 | [03-rotate-trust-anchor](runbooks/03-rotate-trust-anchor.md) |
| `mai_adapter_crash_loop` | `adapter_restarts_total` rate > 3/min | [10-adapter-crash-loop](runbooks/10-adapter-crash-loop.md) |
| `mai_airgap_violation` | any `airgap.violation` audit entry | [13-air-gap-violation](runbooks/13-air-gap-violation.md) |
| `mai_disk_low` | any monitored partition > 85% used | [14-disk-almost-full](runbooks/14-disk-almost-full.md) |
| `mai_backup_failed` | nightly `backup create` or `backup verify` non-zero | [07-back-up-node](runbooks/07-back-up-node.md) |
| `mai_readiness_failed` | `/v1/health/ready` non-200 for > 60s | [INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md) |
| `mai_policy_decision_error_rate` | `policy_decision_total{outcome="error"}` rate above baseline | [INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md) |

Every alert above should page; none of them are "review in the
morning" signals.

## Log sinks

- Structured logs from `mai-api`, `mai-dashboard`, and
  `mai-adapter-manager` go to stderr -> journald. Set
  `MAI_LOG_FORMAT=json` for ingestion.
- Per-adapter Python logs live under `/var/log/mai/adapter-*.log`.
- The audit WAL is **not** a log sink — it is the integrity
  record. Treat it as such; never ship it to log aggregation
  without a path that preserves the chain.

## Connectivity surface

`/v1/system/connectivity` reports the in-process view of every
non-loopback connection attempt MAI's own code path makes. The
shape:

```json
{
  "policy": "strict",
  "loopback_only": true,
  "allowed_egress": [],
  "recent_events": [
    { "ts": "...", "dst": "...", "proto": "...", "allowed": false }
  ]
}
```

In `strict` mode, `allowed_egress` should be empty; any entry
means the operator's profile explicitly permits a specific
destination (e.g. an upstream NTP). Any `recent_events` with
`allowed: false` is the trigger for runbook 13.

## Dashboard

The compliance dashboard (FastAPI under uvicorn, served by
`mai-dashboard.service`) provides a read-only view of:

- Policy module status and recent decisions.
- Audit chain head + last verified checkpoint.
- Trust posture (anchor count, bundle expiry, last verify).
- Capacity (request rate, queue depth, KV utilization).
- Backup history and last verify.

The dashboard is **read-only**. It never edits config, never
mints keys, never imports bundles. Operator actions remain on
the CLI; the dashboard exists so that on-call can answer "what
is happening right now" without shelling in.

## Boundary

No metric, log, or dashboard surface ever includes regulated
payload content. The audit WAL records the *fact* of a request,
the policy decision, and the response classification — never
the text of the request or the response. This is a hard
contract, enforced by the writers; do not add fields that would
leak content.
