# MAI Python SDK Reference

**Project:** Island Mountain Model Abstraction Interface (MAI)
**SDK package:** `mai-sdk-python`
**Audience:** Application developers, acquirer integration engineers
**Status:** Session 45 acquisition documentation
**Source of truth:** `mai-sdk-python/src/mai/`
**Last Updated:** 2026-05-23

The Python SDK is the supported integration surface for MAI. It
covers the full HTTP API documented in
[`API-REFERENCE.md`](API-REFERENCE.md), adds retry / auth / error
handling, and ships sync and async clients with identical namespace
shapes.

Version: 0.2.0 (Session 29 + S44 + BF-6).

---

## Install and quickstart

```bash
pip install mai-sdk-python
```

```python
from mai import MaiClient

client = MaiClient.from_env()           # reads MAI_API_BASE + MAI_API_KEY
client = MaiClient.from_file("mai.toml")  # loads from a config file
client = MaiClient.load()               # tries env then file then defaults

response = client.chat.completions(model="lamprey/fast",
                                   messages=[{"role": "user", "content": "..."}])
```

Async equivalent:

```python
from mai import AsyncMaiClient

async with AsyncMaiClient.from_env() as client:
    response = await client.chat.completions(model="lamprey/fast",
                                             messages=[...])
```

---

## Client construction

| Factory | Reads from |
|---|---|
| `MaiClient.from_env()` | `MAI_API_BASE`, `MAI_API_KEY`, `MAI_TIMEOUT_SECONDS`, `MAI_RETRY_MAX_ATTEMPTS` |
| `MaiClient.from_file(path)` | TOML config with `[client]` block |
| `MaiClient.load()` | env → file (`./mai.toml` then `~/.mai/config.toml`) → defaults |
| `MaiClient(api_key=..., base_url=...)` | Explicit args |

`AsyncMaiClient` mirrors all three factories.

Config shape (`mai/config.py`):

```toml
[client]
base_url = "http://localhost:8080"
api_key  = "im-..."
timeout_seconds = 60.0
verify_tls = true

[client.retry]
max_attempts = 3
backoff_base_seconds = 0.5
jitter = true
```

---

## Namespaces

### `client.models` — model management
| Method | Returns | Notes |
|---|---|---|
| `list(**filters)` | `list[ModelObject]` | Profile-filtered |
| `get(model_id)` | `ModelDetail` | |
| `load(model_id)` | `ModelLoadResponse` | Admin |
| `unload(model_id)` | `ModelUnloadResponse` | Admin |
| `benchmark(model_id, **opts)` | `BenchmarkResult` | Admin; runs 8-metric suite |
| `get_benchmark(model_id)` | `BenchmarkResult` | Cached last run |
| `discover(path=None)` | `ModelDiscoverResponse` | Admin |
| `install(...)` | `ModelInstallResponse` | Admin; streaming upload |
| `remove(model_id)` | `ModelRemoveResponse` | Admin |

### `client.chat` — chat completions
- `client.chat.completions(...)` — non-streaming
- `client.stream_chat(...)` — streaming SSE iterator
- `client.chat.completions(stream=True)` also returns an iterator

### `client.embed` / `client.embeddings`
- `client.embed(input=...)` → `EmbeddingResponse`

### `client.power` — power state
| Method | Returns |
|---|---|
| `get_state()` | `PowerStateResponse` |
| `transition(instance_id, target)` | `PowerTransitionResponse` |

### `client.system` — system observability
- `airgap()` → `AirgapStatusResponse`
- `system_health()` → `SystemHealthResponse`
- `hardware_health()` → `HardwareHealthResponse`

### `client.scheduler` — scheduler telemetry (read-only)
- `metrics()` → `SchedulerMetricsResponse`
- `instance_metrics(id)` → `InstanceMetricsResponse`
- `instance_health(id)` → `InstanceHealthResponse`
- `anomalies()` → `SchedulerAnomaliesResponse`

### `client.updates` — OTA
- `check()`, `download(component, target_version)`, `status()`

### `client.admin` — admin-only
- `list_profiles()`, `get_profile(id)`, `audit_log(...)`, `adapters()`,
  `registry()`, `registry_scan()`

### `client.auth` — Trust Manifold auth (BF-6)
- `exchange_token(subject_id, *, tenant_id, scopes)` →
  `ExchangeTokenResponse`. Local-dev today; production swap is a
  handler-body change with the wire shape unchanged.

### `client.trust` — local trust cache (BF-6)
- `status()` → `TrustStatusResponse`
- `claims()` → `TrustClaimsResponse` (admin)
- `bundle_status()` → `TrustBundleStatus`
- `revocation_status(claim_id)` → `RevocationStatusResponse`

### `client.compliance` — Lamprey surface (S44)
Policy:
- `get_status()` → `ComplianceStatus`
- `get_policies()`, `get_policy(module)`, `update_policy(module, *,
  enabled)`, `reload_policy()`, `apply_template(template)`,
  `enable_module(module)`, `disable_module(module)`

Audit:
- `query_audit(**filters)` → `AuditQueryResponse`
- `get_audit_entry(entry_id)`
- `verify_audit()`, `audit_integrity()`

Reports:
- `list_reports()` → `ComplianceReportList`
- `generate_report(template, *, scope=..., format=...)` →
  `ComplianceReport`
- `get_report(report_id)`, `download_report(report_id)` → `bytes`
- `delete_report(report_id)`

---

## Error hierarchy (`mai/errors.py`)

```
MaiError
├── BadRequestError              # 400
├── AuthenticationError          # 401
│   └── ClaimExpiredError        # 401, trust claim expired
├── PermissionError              # 403
│   └── AirGapViolationError     # 403, air-gap policy refusal
├── NotFoundError                # 404
├── RateLimitError               # 429 (carries retry_after)
├── ServerError                  # 5xx
├── ConnectionError              # network failure
├── TimeoutError                 # request timed out
├── PowerStateUnavailableError   # 503, power state cannot serve
└── TrustCacheStaleError         # local trust cache stale/expired
```

`MaiError.is_retryable` returns `True` for 429 / 5xx / transient
server-tagged types (`rate_limited`, `request_failed`, `overloaded`,
`power_state_unavailable`, `timeout`).

Retry policy (`mai/retry.py`): exponential backoff with full jitter,
default 3 attempts, retries only on `is_retryable` failures.

---

## CLI (`mai/cli.py`)

Installs an entry point named `mai`:

```
mai health
mai chat "hello world" --model lamprey/fast
mai models list
mai models load lamprey/fast
mai models unload lamprey/fast
mai benchmark lamprey/fast
mai power state
```

Reads config from `MAI_API_BASE` + `MAI_API_KEY` or `~/.mai/config.toml`.
Outputs JSON by default; pass `--text` for line-by-line output.

---

## Types module (`mai/types.py`)

Highlights relevant to S44 + BF-6:

| Type | Shape source | Used by |
|---|---|---|
| `TrustStatusResponse` | `/v1/trust/status` JSON | `client.trust.status()` |
| `TrustClaimSnapshot` | single claim | `TrustClaimsResponse` |
| `TrustClaimsResponse` | `/v1/trust/claims` | `client.trust.claims()` |
| `TrustBundleStatus` | `/v1/trust/bundle_status` | `client.trust.bundle_status()` |
| `RevocationStatusResponse` | `/v1/trust/revocation_status` | `client.trust.revocation_status(...)` |
| `ExchangeTokenResponse` | `/v1/auth/exchange_token` | `client.auth.exchange_token(...)` |
| `ComplianceStatus` | `/v1/compliance/status` | `client.compliance.get_status()` |
| `ComplianceModuleStatus` | per-module entry | `ComplianceStatus.modules[]` |
| `ComplianceIntegrity` | `/v1/compliance/audit/integrity` | `client.compliance.audit_integrity()` |
| `AuditRow` | `/v1/compliance/audit` row | `AuditQueryResponse.entries[]` |
| `AuditQueryResponse` | `/v1/compliance/audit` body | `client.compliance.query_audit(...)` |
| `ComplianceReport` | `/v1/compliance/reports/{id}` | `client.compliance.get_report(...)` |
| `ComplianceReportList` | `/v1/compliance/reports` | `client.compliance.list_reports()` |
| `BenchmarkResult` | S29 endpoint | `client.models.benchmark(...)` |
| `AirgapStatusResponse` | `/v1/system/airgap` | `client.system.airgap()` |
| `SchedulerMetricsResponse` | `/v1/scheduler/metrics` | `client.scheduler.metrics()` |

The `TrustClaim` type (rich shape predating BF-6) is preserved for
forward compat with callers that built against the Session 29 stub.

---

## Async parity

`mai.async_client.AsyncMaiClient` exposes the same namespace tree:
`client.models`, `client.chat`, `client.embed`, `client.scheduler`,
`client.system`, `client.power`, `client.updates`, `client.admin`,
`client.auth`, `client.trust`, `client.compliance`. Method names and
return types are identical. The implementation lives entirely in
`async_client.py` rather than wrapping the sync namespaces — this
keeps the call paths simple and lets each side optimise transport
independently.

`AsyncMaiClient` is an `async with` context manager; calling
`await client.aclose()` flushes the underlying `httpx.AsyncClient`.

---

## Streaming

Sync iteration:

```python
for event in client.stream_chat(model="lamprey/fast",
                                messages=[{"role": "user", "content": "..."}]):
    print(event.delta_text, end="", flush=True)
```

Async iteration:

```python
async for event in client.chat.stream(model="...", messages=[...]):
    print(event.delta_text, end="", flush=True)
```

Backpressure is handled by the SDK's underlying SSE parser; the
caller controls flow by how fast they iterate.

---

## Test posture

- `mai-sdk-python/tests/` — 94 SDK tests covering retry, error
  mapping, type round-trips, namespace wiring, BF-6 + S44 surfaces.
- `apps/openbao-trust-demo/tests/` — 17 scaffold tests exercising
  `client.trust.*`, `client.auth.exchange_token`, end-to-end Trust
  Manifold flow.
- `apps/operator/tests/` — 12 scaffold tests exercising the trust
  panel + system surfaces.

Run:

```powershell
cd mai-sdk-python; pytest
PYTHONPATH=mai-sdk-python/src python -m pytest apps/openbao-trust-demo/tests/
```

---

## What an acquirer can verify in 15 minutes

1. Read `mai/_namespaces.py` — every public method on every namespace
   is in one file.
2. Read `mai/errors.py` — the full exception tree fits on one screen.
3. Run `pytest mai-sdk-python/tests/` — 94 green tests.
4. Walk the openbao-trust-demo scaffold's `main.py` — sees auth →
   exchange → trust status → inference → audit query end to end.
5. Run `mai health` against a local `mai-api` server.

The SDK is the integration contract. If a method appears in
`_namespaces.py`, it is wired, tested, and exposed in the dashboard
or the demo scaffolds.
