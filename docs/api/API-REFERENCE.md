# MAI HTTP API Reference

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Integration engineers, SDK authors, acquirer evaluators
**Status:** Session 45 acquisition documentation
**Source of truth:** `mai-api/src/routes.rs` (the table here mirrors
the live router)
**Last Updated:** 2026-05-23

This is the live shape of the MAI REST surface as of Session 44 + BF-6.
All routes are mounted under `/v1` and protected by the auth
middleware described in [`SECURITY.md`](../compliance/SECURITY.md) unless
explicitly noted.

For SDK-level access, see [`SDK-REFERENCE.md`](SDK-REFERENCE.md).
For the Python SDK shape, see `mai-sdk-python/`.
For a machine-readable OpenAPI contract, see [`api/openapi.yaml`](api/openapi.yaml).

---

## Auth model in one paragraph

Every non-health endpoint requires a `X-IM-Auth-Token: im-...` header
carrying a 32-byte hex-encoded API key with `im-` prefix. Keys are
stored as SHA3-256 hashes in `config/auth_keys.toml`. The middleware
extracts the caller's profile (Admin / Adult / Teen / Child / Guest)
and stamps it onto the request. Handlers call `check_permission` to
enforce per-action authorisation. Trust-claim-aware endpoints (BF-6
and S44) additionally consult the local trust cache.

Standard error shape (`mai-api/src/errors.rs`):

```json
{
  "error": {
    "code": "MAI-A101",
    "message": "human-readable summary",
    "type": "authentication_failed",
    "retry_after_seconds": null,
    "request_id": "..."
  }
}
```

---

## Inference routes

### `POST /v1/chat/completions`

Streaming-capable chat completion. Honours model alias resolution via
`mai-scheduler/src/aliases.rs`. Stream control by `"stream": true`.

| Field | Notes |
|---|---|
| Permission | `inference` |
| Streaming | SSE if `stream: true`; otherwise JSON |
| Request | `ChatCompletionRequest` (`mai-api/src/types.rs`) |
| Response | `ChatCompletionResponse` or SSE event stream |
| Errors | 400 invalid request, 403 model not in profile scope, 503 overloaded |

### `POST /v1/completions`

Legacy alias to `chat_completions`. Same handler, same shape; only
exists for SDK compat with code that hasn't migrated.

### `POST /v1/embeddings`

Embedding generation, batched.

| Field | Notes |
|---|---|
| Permission | `inference` |
| Request | `EmbeddingRequest` with `input: string \| string[]` |
| Response | `EmbeddingResponse` with `data: { embedding: number[], index }[]` |

### `POST /v1/generate/structured`

Schema-guided structured output (JSON, EBNF). Honours `response_format`.

### `POST /v1/generate/function_call`

Tool-calling surface. Returns a `ToolCall` array; the caller
dispatches and re-submits with the tool results.

---

## Model routes

### `GET /v1/models`

Lists models the caller's profile can see (profile-filtered).

### `GET /v1/models/{model_id}`

Detail view: tier, VRAM, alias list, current power state, instance
membership.

### `DELETE /v1/models/{model_id}`

Admin-only. Equivalent to `POST /v1/models/{model_id}/remove`.

### `POST /v1/models/{model_id}/load`

Admin-only. Triggers a load on the chosen instance per scheduler
placement.

### `POST /v1/models/{model_id}/unload`

Admin-only. Releases instance memory.

### `POST /v1/models/{model_id}/benchmark`

Admin-only. Runs the standard 8-metric benchmark suite against the
loaded model.

### `GET /v1/models/{model_id}/benchmark`

Last benchmark result; cached.

### `POST /v1/models/discover`

Admin-only. Scans the configured search paths for installable model
packages.

### `POST /v1/models/install`

Admin-only. Streaming upload (raw body, content-length required).
Validates manifest, hashes, and resumable shard plan.

### `POST /v1/models/{model_id}/remove`

Admin-only. Removes a model from the registry; unloads first if
loaded.

---

## OTA update routes (Session 25)

### `GET /v1/updates/check`

Admin-only. Returns available updates with shard plan.

### `POST /v1/updates/download`

Admin-only. Starts a background differential download.

### `GET /v1/updates/status`

Admin-only. Current download progress.

---

## Health routes (auth exempt)

### `GET /v1/health`

Aggregate health: adapters, hardware, scheduler, vault.

### `GET /v1/health/adapters`

Per-adapter status + last heartbeat.

### `GET /v1/health/hardware`

GPU / CPU / memory / disk / thermal.

### `GET /v1/health/system`

Adapter-rollup aggregator (J-13). Fans out a live `health_check`
probe to every registered adapter and folds the per-adapter
verdicts into a single `overall` field. Used by production
monitoring.

Response shape:

```json
{
  "overall": "ok",
  "adapters": {
    "ollama": {
      "status": "ok",
      "latency_ms": 12,
      "process_state": "running",
      "detail": { "uptime_ms": 9001, "requests_served": 42 }
    }
  },
  "ts": "2026-05-24T19:42:08+00:00"
}
```

- `overall` is `ok` when every adapter is Healthy; `degraded` when
  any adapter is Degraded or in a transient process state
  (`starting` / `restarting`); `down` when any adapter is
  Unavailable, crashed, stopped, never started, or its
  `health_check` errored.
- `latency_ms` is the wall-clock duration of the per-adapter
  probe (0 for adapters whose status was derived from the
  process state without an IPC call).
- An empty adapter registry returns `overall: "ok"` (vacuous).

### `GET /v1/health/resources`

Disk, RAM, and CPU utilization percentages — the body previously
served at `/v1/health/system` (renamed by J-13 to free the old
path for the adapter rollup).

---

## System routes

### `GET /v1/system/airgap`

Air-gap status. See [`AIR-GAP-BRIEF.md`](../product/AIR-GAP-BRIEF.md).

### `GET /v1/power`, `GET /v1/power/state`

Current power state across instances.

### `POST /v1/power/transition`

Admin-only or scheduler-only. Transitions a target instance between
Deep Vault Sleep / Sentinel / Full Inference.

### `GET /v1/registry`

Lists registered models with affinity ordering.

### `POST /v1/registry/scan`

Admin-only. Re-scans the registry from disk.

### `GET /v1/adapters`

Lists adapter processes with PID, model, heartbeat age.

### `GET /v1/audit/log`

Profile-filtered audit log view. Distinct from the compliance audit
log (`/v1/compliance/audit`) — this is the legacy MAI audit chain
(`mai-vault/src/audit.rs`).

### `GET /v1/profiles`

Profile listing (Admin only).

### `GET /v1/profiles/{profile_id}`

Profile detail.

---

## Scheduler telemetry (Session 20)

### `GET /v1/scheduler/metrics`

Cluster-wide scheduler counters: admit/preempt/eviction rates, P50/95/99
placement latency, decision-cache hit rate.

### `GET /v1/scheduler/instances/{id}/metrics`

Per-instance counters.

### `GET /v1/scheduler/instances/{id}/health`

Per-instance health score.

### `GET /v1/scheduler/anomalies`

Recent anomalies surfaced by the metrics feedback loop.

---

## Streaming

### `GET /v1/ws` (WebSocket upgrade)

Token-stream channel. Frame format documented in
`mai-api/src/streaming/ws.rs`. Auth handshake on the first frame.

SSE streaming is delivered inline on `/v1/chat/completions` when
`stream: true`.

---

## Trust routes (BF-6, Session 44)

These read the local trust cache. Metadata only — no prompt,
completion, or embedding content flows through these endpoints.

### `GET /v1/trust/status`

Consolidated trust mode.

```json
{
  "mode": "connected",
  "bundle_version": "2026.05.22.001",
  "claim_count": 42,
  "offline_backlog": 0,
  "last_refresh": "2026-05-22T23:14:51Z",
  "switch_engaged": false
}
```

Possible `mode` values: `connected`, `degraded`, `stale_not_expired`,
`expired`, `air_gapped`, `not_provisioned`.

### `GET /v1/trust/claims`

Admin-only. Lists active claims.

### `GET /v1/trust/bundle_status`

Current bundle version, signer, fetched-at, valid-until.

### `GET /v1/trust/revocation_status?claim_id=...`

Revocation snapshot for a specific claim ID.

### `POST /v1/auth/exchange_token`

Mints a session token from a subject identity. Local-dev stub today;
production replaces handler body. Wire shape:

```json
{
  "subject_id": "user:12345",
  "tenant_id": "demo-tenant",
  "scopes": ["hipaa", "ocap"]
}
```

Response:

```json
{
  "token": "im-...",
  "expires_at": "2026-05-22T12:30:00Z",
  "claim": { ... }
}
```

---

## Compliance routes (Session 44)

### Policy management

| Method | Path | Permission | Purpose |
|---|---|---|---|
| GET | `/v1/compliance/status` | `view_audit` | Module health + active template + policy version |
| GET | `/v1/compliance/policies` | `view_audit` | List enabled modules |
| GET | `/v1/compliance/policies/{module}` | `view_audit` | Read module rules |
| PUT | `/v1/compliance/policies/{module}` | `manage_models` | Update module config |
| POST | `/v1/compliance/policies/reload` | `manage_models` | Reload from disk |
| POST | `/v1/compliance/policies/template` | `manage_models` | Apply template (Standard/Healthcare/Defense/TribalGovernment) |
| POST | `/v1/compliance/modules/{name}/enable` | `manage_models` | Enable module |
| POST | `/v1/compliance/modules/{name}/disable` | `manage_models` | Disable module |

### Audit access

| Method | Path | Permission | Purpose |
|---|---|---|---|
| GET | `/v1/compliance/audit` | `view_audit` | Query audit entries (filter by tenant, module, decision, time) |
| GET | `/v1/compliance/audit/{id}` | `view_audit` | Single entry detail |
| GET | `/v1/compliance/audit/verify` | `view_audit` | Verify chain links + periodic signatures |
| GET | `/v1/compliance/audit/integrity` | `view_audit` | Detailed integrity report |

### Reports

| Method | Path | Permission | Purpose |
|---|---|---|---|
| GET | `/v1/compliance/reports` | `view_audit` | List generated reports |
| POST | `/v1/compliance/reports/generate` | `manage_models` | Generate from template (HIPAA / ITAR / OCAP / SystemActivity / MonthlyDigest / Custom) |
| GET | `/v1/compliance/reports/{id}` | `view_audit` | Report metadata + status |
| GET | `/v1/compliance/reports/{id}/download` | `view_audit` | Body bytes (JSON / HTML / CSV / Text) + signature header |
| DELETE | `/v1/compliance/reports/{id}` | `manage_models` | Remove (refuses if `protected = true`) |

### Live feed

### `GET /v1/compliance/feed`

Server-Sent Events stream of compliance events:

| Event type | Payload |
|---|---|
| `decision_made` | composer aggregate + reason codes |
| `policy_changed` | module + new state + actor |
| `module_state_changed` | module + enabled/disabled |
| `violation_detected` | threshold trigger + severity |

The dashboard's Alerts page consumes this stream live.

---

## Permissions

Defined in `mai-api/src/auth.rs`. Common permissions referenced
above:

| Permission | Role floor |
|---|---|
| `inference` | Guest (sentinel only), Child/Teen (filtered), Adult, Admin |
| `list_models` | Adult |
| `manage_models` | Admin |
| `power_control` | Admin |
| `manage_profiles` | Admin |
| `view_audit` | Admin (or service identity with audit read scope) |

---

## Error code reference (`mai-api/src/errors.rs`)

A subset that appears frequently in BF-6 / S44 endpoints:

| Code | HTTP | Meaning |
|---|---|---|
| `MAI-A101` | 401 | Invalid or missing API key |
| `MAI-A102` | 401 | Trust claim expired |
| `MAI-A201` | 403 | Permission denied |
| `MAI-A202` | 403 | Air-gap policy violation |
| `MAI-A301` | 403 | Trust cache stale / unverified |
| `MAI-R401` | 429 | Rate limit exceeded |
| `MAI-S501` | 503 | Power state cannot serve |
| `MAI-S502` | 503 | Scheduler overloaded |
| `MAI-V601` | 400 | Validation failure |

---

## Source-of-truth navigation

When the table in this document and the live router disagree, the
router wins. The mapping:

- `mai-api/src/routes.rs` — `build_router()` is the authoritative
  enumeration.
- `mai-api/src/handlers/{inference,models,health,system,telemetry,trust,compliance}.rs`
  — per-route handler bodies.
- `mai-api/src/auth.rs` — permission enum + `check_permission`.
- `mai-api/src/errors.rs` — error code definitions.
- `mai-api/tests/compliance_integration.rs` — 17 HTTP integration
  tests covering the BF-6 / S44 surface end-to-end.
- `mai-api/tests/http_integration.rs` — base REST coverage.

Read `routes.rs` first; everything else fans out from there.
