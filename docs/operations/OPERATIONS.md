# Day-2 Operations

Routine operating cadence for a `ship` appliance. This is the
"what does the operator actually do, day to day" doc; for
specific procedures, follow the runbook links inline. For
incident-class events see
[INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md).

## Routine cadence

| Cadence | Action | Procedure |
|---|---|---|
| Per boot | Verify ship validator passes | [RELEASE-GATES.md](../releases/RELEASE-GATES.md) |
| Per boot | Confirm health endpoints reachable | `curl /v1/health/{live,ready}` |
| Daily | Healthcheck timer runs (automatic) | unit `mai-healthcheck.timer` |
| Daily | Nightly backup runs (automatic) | [07-back-up-node](runbooks/07-back-up-node.md) |
| Daily | Daily audit verify (automatic, via healthcheck) | [05-verify-audit-chain](runbooks/05-verify-audit-chain.md) |
| Weekly | Manual full-chain audit verify | runbook 05 |
| Weekly | Review alerts triaged in the last 7 days | [OBSERVABILITY.md](OBSERVABILITY.md) |
| Monthly | Trust anchor expiry review | runbook 03 |
| Monthly | Bundle expiry review | runbook 11 |
| Quarterly | API key rotation | runbook 02 |
| Quarterly | Compliance report generation | runbook 06 |
| Quarterly | Restore drill against an isolated target | runbook 08 |
| Annually | Trust anchor rotation | runbook 03 |
| As needed | Policy bundle update | runbook 04 |
| As needed | Package upgrade | [UPGRADE-ROLLBACK.md](UPGRADE-ROLLBACK.md) |

## Daily operator checks

Five-minute morning pass:

```bash
sudo systemctl is-active mai-api.service mai-dashboard.service \
                          mai-adapter-manager.service \
                          mai-healthcheck.timer
curl -fsS http://127.0.0.1:8420/v1/health/ready | jq -r .status
sudo journalctl -u mai-api.service --since "24h ago" \
                --grep '^(WARN|ERROR)' | head
sudo -u mai mai-admin audit verify \
     --wal-dir /var/lib/mai/audit --quiet
```

All four should report green. Any warning surfaces the matching
runbook from the index in [runbooks/README.md](runbooks/README.md).

## Service lifecycle

The unit set is fixed:

- `mai-api.service` — REST + gRPC inference + governance.
  `ExecStartPre` runs `mai-ship-validate`; non-zero exit blocks
  startup. Restart is `on-failure` with backoff.
- `mai-dashboard.service` — compliance dashboard (FastAPI under
  uvicorn). Read-only against the API.
- `mai-adapter-manager.service` — per-backend Python adapter
  supervisor. Restart-on-failure with a bounded counter.
- `mai-healthcheck.service` + `mai-healthcheck.timer` — daily
  audit verify, backup, prune, and alert emission.

Operator actions:

```bash
sudo systemctl status   mai-api.service
sudo systemctl restart  mai-api.service   # interrupts SSE/WS
sudo systemctl reload   mai-api.service   # rereads config, keeps connections
sudo systemctl stop     mai-api.service
```

`reload` re-reads `/etc/mai/auth_keys.toml`,
`/etc/mai/policies/`, and the dashboard log config. It does not
re-read `/etc/mai/profile.toml` — profile changes require a
full restart and a fresh `mai-ship-validate`.

## Endpoint cheat sheet

The full API surface is in [API-REFERENCE.md](../api/API-REFERENCE.md);
day-to-day operator endpoints:

```bash
# Liveness — no auth required
curl http://127.0.0.1:8420/v1/health/live

# Readiness — admin token; gates traffic
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/health/ready

# Adapter inventory
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/health/adapters

# System + hardware
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/system/info
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/system/hardware

# Trust and air-gap posture
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/system/trust/status
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/system/connectivity

# Production readiness — same shape as mai-ship-validate
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/system/production-readiness

# Compliance posture
curl -H "X-IM-Auth-Token: $T" http://127.0.0.1:8420/v1/compliance/policy/status
```

## Configuration touchpoints

`/etc/mai/` is the operator's source of truth. Files and what
they govern:

| File | Reread on | Notes |
|---|---|---|
| `profile.toml` | restart only | `mai-ship-validate` is the gate |
| `auth_keys.toml` | reload + restart | Hashes only; never plaintext |
| `policies/` | bundle import | Use runbook 04, not manual edits |
| `trust-anchors/` | reload | `.pub` files; `0640 root:mai` |
| `dashboard-logging.json` | dashboard restart | uvicorn log config |
| `backup-retention.toml` | next healthcheck tick | Defaults documented in runbook 07 |

`/var/lib/mai/` is the daemon's source of truth. Do not edit
its contents by hand — the audit chain depends on the writer
having exclusive ownership of those paths.

## Capacity signals

Watch:

- `/v1/system/hardware` — VRAM headroom and topology health.
- `/v1/metrics` (Prometheus) — request latency, queue depth,
  eviction rate. See [OBSERVABILITY.md](OBSERVABILITY.md).
- `df -h /var/lib/mai /var/backups/mai` — disk headroom; the
  healthcheck emits `disk_low` at 85% but the operator should
  notice before that.
- `nvidia-smi` — driver / hardware health at the host layer.

## Boundary

Day-2 operations stop at policy. Policy bundle authoring,
compliance module changes, and signing-key custody are not
operator activities; they happen upstream of the appliance.
See [TRUST-BRIDGE-PRODUCTION.md](../compliance/TRUST-BRIDGE-PRODUCTION.md)
for the boundary.
