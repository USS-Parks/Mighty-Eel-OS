# Error Path Audit (J-08, Workstream W4)

**Session:** J-08 (DOUGHERTY lane, response to John Dougherty 2026-05-24)
**Scope:** every handler module under `mai-api/src/handlers/`, plus the rate-limit middleware audit (SEC-011) and the JSON schema-validation audit (SEC-012).
**Ground truth:** the source files at workspace HEAD as of this commit. Every verdict cell traces back to a specific function in the module column.
**Date:** 2026-05-24

---

## 1. Error taxonomy (the standard)

`mai-api/src/errors.rs` defines `ApiError` with 22 variants across five code families:

| Family | Range | Meaning |
|:--|:--|:--|
| `MAI-1XXX` | 1001-1005 | Request errors (bad input, validation, content-type, size, timeout) |
| `MAI-2XXX` | 2001-2004 | Model errors (not found, unavailable, incompatible, loading) |
| `MAI-3XXX` | 3001-3005 | System errors (overloaded, internal, hardware, unavailable, adapter crashed) |
| `MAI-4XXX` | 4001-4005 | Auth errors (permission, unauthorized, profile missing, bad token, rate-limited) |
| `MAI-5XXX` | 5001-5003 | Config errors (config, air-gap, endpoint disabled) |

`IntoResponse` is implemented on `ApiError` (`errors.rs:225-262`). Every variant produces the canonical body:

```json
{ "error": { "code": "MAI-XYYY", "message": "...", "type": "..." } }
```

with `retry_after_seconds` added on `MAI-4005` and a `Retry-After` header on 429 responses (RFC 7231 §7.1.3). `From<mai_core::CoreError>` (`errors.rs:266-289`) maps backend-layer errors, and `sanitize_error_detail` (`errors.rs:294-315`) scrubs any leaked backend name (`ollama`, `vllm`, `llama.cpp`, …) before it can reach a client. The acceptance gate for every handler in §2 is: does it produce an `ApiError` (or a `Result<_, ApiError>`) on every fallible path, or does it have a silent panic/unwrap/swallow that a hostile or merely unlucky input could trip?

---

## 2. Per-handler audit

`git ls-files mai-api/src/handlers/` returns ten modules. Each row below names every handler in the module, its route(s) in `routes.rs`, the error variants the body can produce, and a verdict (`PASS` or `FIX-NEEDED`).

### 2.1 `compliance.rs` (787 lines, 16 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `list_policies` | `GET /v1/compliance/policies` | `PermissionDenied` via `check_permission("view_audit")` | PASS |
| `get_policy` | `GET /v1/compliance/policies/{module}` | `PermissionDenied`, `ValidationFailed` (parse_module_id) | PASS |
| `update_policy` | `PUT /v1/compliance/policies/{module}` | `PermissionDenied`, `ValidationFailed` | PASS |
| `reload_policy` | `POST /v1/compliance/policies/reload` | `PermissionDenied` | PASS |
| `apply_template` | `POST /v1/compliance/policies/template` | `PermissionDenied`, `ValidationFailed` | PASS |
| `compliance_status` | `GET /v1/compliance/status` | none — always Ok | PASS |
| `enable_module` / `disable_module` | `POST .../modules/{name}/{enable,disable}` | `PermissionDenied`, `ValidationFailed` | PASS |
| `query_audit` | `GET /v1/compliance/audit` | `PermissionDenied`, `ValidationFailed` (module/decision) | PASS |
| `get_audit_entry` | `GET /v1/compliance/audit/{id}` | `PermissionDenied`, `ModelNotFound` | PASS |
| `audit_integrity` | `GET /v1/compliance/audit/integrity` | `PermissionDenied` | PASS |
| `verify_audit` | `GET /v1/compliance/audit/verify` | `PermissionDenied` (chain error surfaced in body, not as 500 — by design, the dashboard renders a badge) | PASS |
| `list_reports` | `GET /v1/compliance/reports` | `PermissionDenied` | PASS |
| `generate_report` | `POST /v1/compliance/reports/generate` | `PermissionDenied`, `ValidationFailed` (range, format), `map_report_error` (NotFound/ValidationFailed/InternalError) | PASS |
| `get_report` | `GET /v1/compliance/reports/{id}` | `PermissionDenied`, `ModelNotFound` | PASS |
| `download_report` | `GET /v1/compliance/reports/{id}/download` | `PermissionDenied`, `ModelNotFound`, `ModelUnavailable` (body pruned) | PASS |
| `delete_report` | `DELETE /v1/compliance/reports/{id}` | `PermissionDenied`, `map_report_error` | PASS |
| `compliance_feed` | `GET /v1/compliance/feed` | `PermissionDenied`; stream errors are `Infallible` by design (SSE is best-effort) | PASS |

**Module verdict:** PASS. Dedicated `map_report_error` helper (`compliance.rs:125-140`) centralises report taxonomy.

### 2.2 `health.rs` (405 lines, 7 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `aggregate_health` | `GET /v1/health` | none — always 200 | PASS |
| `adapter_health` | `GET /v1/health/adapters` | none | PASS |
| `hardware_health` | `GET /v1/health/hardware` | none | PASS |
| `system_health` | `GET /v1/health/system` | none | PASS |
| `live_probe` | `GET /v1/health/live` | none — always 200 by definition | PASS |
| `ready_probe` | `GET /v1/health/ready` | returns 503 + reasons body when audit writer unresponsive / hardware alert is Shutdown | PASS |
| `production_probe` | `GET /v1/health/production` | returns 503 + reasons body for any production-invariant violation | PASS |

**Module verdict:** PASS. The probe endpoints intentionally don't use `ApiError`; their wire shape is the canonical `ProbeResponse { status, reasons[] }` paired with the HTTP code (see `health.rs:217-223`). Operators key off the status code, humans key off the reasons list.

### 2.3 `inference.rs` (689 lines, 4 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `chat_completions` | `POST /v1/chat/completions`, `POST /v1/completions` | `PermissionDenied`, `ValidationFailed` (param ranges), scheduler → `ModelUnavailable` / `SystemOverloaded` / `InternalError`, **adapter → `AdapterCrashed` / `RequestTimeout` / `InternalError`** | PASS |
| `embeddings` | `POST /v1/embeddings` | `PermissionDenied`, `ValidationFailed` (empty input), scheduler → `ModelUnavailable`, **adapter → `InternalError` only** | FIX-NEEDED |
| `structured_generation` | `POST /v1/generate/structured` | `PermissionDenied`, `ValidationFailed`, scheduler → `ModelIncompatible`, **adapter → `InternalError` only** | FIX-NEEDED |
| `function_call` | `POST /v1/generate/function_call` | `PermissionDenied`, `ValidationFailed`, scheduler → `ModelIncompatible`, **adapter → `InternalError` only** | FIX-NEEDED |

**Module verdict:** FIX-NEEDED (scope is small — three sites). `chat_completions` discriminates `FrameworkError::ProcessCrashed` / `ResponseTimeout` / other (`inference.rs:115-126`); the other three handlers collapse every adapter error to `InternalError` (`inference.rs:249-252`, `inference.rs:364-368`, `inference.rs:514-518`). A backend crash on `/v1/embeddings` currently returns 500 instead of the more precise 503 `AdapterCrashed`. **This session brings the three to parity with `chat_completions`.**

### 2.4 `metrics.rs` (35 lines, 1 handler)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `prometheus_metrics` | `GET /v1/metrics` | none — exposition is infallible | PASS |

**Module verdict:** PASS. Auth-exempt by design (`auth.rs:55`); the redaction guarantee on `metrics::sanitize_label_value` keeps secrets out of the body even if a counter is mis-instrumented.

### 2.5 `models.rs` (473 lines, 8 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `list_models` | `GET /v1/models` | none — visibility filtering is silent | PASS |
| `get_model` | `GET /v1/models/{model_id}` | `ModelNotFound` (manifest absent OR status absent) | PASS |
| `load_model` | `POST /v1/models/{model_id}/load` | `PermissionDenied`, `ModelNotFound`, `ModelUnavailable` | PASS |
| `unload_model` | `POST /v1/models/{model_id}/unload` | `PermissionDenied`, `ModelNotFound`, `ModelUnavailable` | PASS |
| `benchmark_model` | `POST /v1/models/{model_id}/benchmark` | `PermissionDenied`, `ModelUnavailable` | PASS |
| `get_model_benchmark` | `GET /v1/models/{model_id}/benchmark` | `PermissionDenied`, `ModelUnavailable` | PASS |
| `discover_packages` | `POST /v1/models/discover` | `PermissionDenied`; per-drive errors are returned inside the response body so callers see all of them | PASS |
| `install_handler_raw` | `POST /v1/models/install` | `extract_profile` → `Unauthorized`/`TokenInvalid`, `check_permission` → `PermissionDenied`, JSON extraction → `BadRequest`, installer → `BadRequest` (with installer-provided context) | PASS |
| `remove_model_handler` | `POST /v1/models/{model_id}/remove` and `DELETE /v1/models/{model_id}` | `PermissionDenied`, `ModelUnavailable` | PASS |

**Module verdict:** PASS. The `install_handler_raw` site uses the manual extraction shim around the axum 0.7/0.8 trait clash (documented in the file header); every fallible step still produces an `ApiError`.

### 2.6 `system.rs` (412 lines, 8 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `production_readiness` | `GET /v1/system/production-readiness` | `PermissionDenied`, `ValidationFailed` (no ship profile loaded → MAI-1002, 422) | PASS |
| `get_airgap_status` | `GET /v1/system/airgap` | none | PASS |
| `get_power_state` | `GET /v1/power`, `GET /v1/power/state` | none | PASS |
| `power_transition` | `POST /v1/power/transition` | `PermissionDenied`, `ValidationFailed` (unknown action, `PowerError::InvalidTransition`, `PowerError::GuardFailed`), `InternalError` for other `PowerError` variants | PASS |
| `get_registry` | `GET /v1/registry` | none | PASS |
| `registry_scan` | `POST /v1/registry/scan` | `PermissionDenied` | PASS |
| `list_adapters` | `GET /v1/adapters` | none | PASS |
| `get_audit_log` | `GET /v1/audit/log` | `PermissionDenied`, `InternalError` (writer error logged then mapped) | PASS |
| `list_profiles` / `get_profile` | `GET /v1/profiles*` | `PermissionDenied` (only on `get_profile` for cross-profile access) | PASS |

**Module verdict:** PASS. `power_transition` enumerates the four backend error variants explicitly (`system.rs:122-133`).

### 2.7 `telemetry.rs` (112 lines, 4 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `scheduler_metrics` | `GET /v1/scheduler/metrics` | serde failure → `InternalError` | PASS |
| `instance_metrics` | `GET /v1/scheduler/instances/{id}/metrics` | none — returns null health for unknown instance with a `message` field | PASS |
| `instance_health` | `GET /v1/scheduler/instances/{id}/health` | serde failure → `InternalError`; unknown instance returns a null-score JSON body | PASS |
| `scheduler_anomalies` | `GET /v1/scheduler/anomalies` | serde failure → `InternalError` | PASS |

**Module verdict:** PASS.

### 2.8 `trust.rs` (334 lines, 5 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `get_trust_status` | `GET /v1/trust/status` | none | PASS |
| `list_claims` | `GET /v1/trust/claims` | `PermissionDenied` (`view_audit`) | PASS |
| `bundle_status` | `GET /v1/trust/bundle_status` | none | PASS |
| `revocation_status` | `GET /v1/trust/revocation_status` | `ValidationFailed` (empty `claim_id` query) | PASS |
| `exchange_token` | `POST /v1/auth/exchange_token` | `ValidationFailed` (empty `subject_id`), `ServiceUnavailable` (`OpenBaoBridge` not wired), `EndpointDisabled` (mode = `Disabled`) | PASS |

**Module verdict:** PASS. `exchange_token` explicitly fails closed on `OpenBaoBridge` rather than falling through to the synthetic mint (`trust.rs:246-252`) — this is the correct behaviour for a production profile.

### 2.9 `updates.rs` (180 lines, 3 handlers)

| Handler | Route | Errors produced | Verdict |
|:--|:--|:--|:--|
| `check_updates` | `GET /v1/updates/check` | `PermissionDenied` (`manage_models`) | PASS |
| `start_update_download` | `POST /v1/updates/download` | `PermissionDenied`, `ValidationFailed` (non-HTTPS URL) | PASS |
| `update_status` | `GET /v1/updates/status` | `PermissionDenied` | PASS |

**Module verdict:** PASS. The HTTPS check (`updates.rs:67-71`) is a deliberate validation gate.

### 2.10 `mod.rs` (16 lines)

Re-export only. No handlers. N/A.

### 2.11 Summary

| Module | Handlers | PASS | FIX-NEEDED |
|:--|--:|--:|--:|
| compliance.rs | 16 | 16 | 0 |
| health.rs | 7 | 7 | 0 |
| inference.rs | 4 | 1 | **3** |
| metrics.rs | 1 | 1 | 0 |
| models.rs | 8 | 8 | 0 |
| system.rs | 8 | 8 | 0 |
| telemetry.rs | 4 | 4 | 0 |
| trust.rs | 5 | 5 | 0 |
| updates.rs | 3 | 3 | 0 |
| **TOTAL** | **56** | **53** | **3** |

53 of 56 handlers PASS as-shipped. The three FIX-NEEDED rows are all in `inference.rs` and all share the same shape — adapter-layer `FrameworkError` collapses to `InternalError` instead of discriminating `ProcessCrashed` and `ResponseTimeout`. Fix applied in this session: §5.1.

---

## 3. Rate-limit audit (SEC-011)

GitDoctor flagged `SEC-011: No rate limiting on API routes`. **Verdict: FALSE POSITIVE.** Rate limiting is implemented at the auth middleware layer.

### 3.1 Implementation

- `mai-api/src/auth.rs:60-336` defines `RateLimiter` (sliding-window, per-key-hash, configurable threshold).
- `RateLimiter::default_per_minute()` defaults to **60 requests / 60 seconds** (`auth.rs:188-198`).
- `auth_middleware` (`auth.rs:404-490`) calls `rate_limiter.check_rate_limit(&key_hash)` on every authenticated request (`auth.rs:446`).
- On exceed, returns `ApiError::RateLimited(retry_after)` which renders as HTTP `429 Too Many Requests` with a `Retry-After: <seconds>` header (`errors.rs:225-261`).
- `metrics_middleware` increments `mai_rate_limited_total{route}` on every 429 (`middleware.rs:156-160`), so operators can graph throttling per route.
- `AuthState::with_rate_limit(store, max_requests, window_seconds)` (`auth.rs:373-378`) lets operators tune the threshold via the `rate_limit_per_minute` profile field documented at `auth.rs:553`.

### 3.2 Per-route coverage

The auth middleware is wired in `routes.rs:288-291` as the inner middleware layer on the merged router. The rate limit therefore applies to **every** route except those in `AUTH_EXEMPT_PREFIXES` (`auth.rs:55`):

| Route prefix | Auth | Rate-limited | Rationale |
|:--|:--:|:--:|:--|
| `/v1/health/*` | exempt | exempt | Operator-local probes (systemd watchdog, k8s probes, LB health). Must succeed even when the API key store is unhealthy. |
| `/v1/metrics` | exempt | exempt | Host-local Prometheus scraper; redaction in `metrics::sanitize_label_value` keeps the body secret-free. |
| Every other route | required | required | All inference, model, compliance, trust, system, telemetry, updates, audit routes are protected. |

### 3.3 Verdict

SEC-011 is **mitigated**, not absent. No code change needed in this session. The GitDoctor flag fires because the scanner heuristic looks for `tower-governor` / `tower::limit::RateLimitLayer` imports, which are absent — but the in-tree `RateLimiter` is keyed on API-key hash, which is the right granularity for a single-tenant air-gapped appliance. The rescan (J-14) should clear this, or we add a refutation row in the response doc (J-15) if the heuristic still fires.

---

## 4. Schema-validation audit (SEC-012)

GitDoctor flagged `SEC-012: Missing schema validation on input`. `grep -rn deny_unknown_fields mai-api/src/` returns zero hits. **Verdict: REAL GAP** — every JSON request body silently accepts unknown fields. For inference endpoints this is intentional (OpenAI-compat SDKs add fields liberally); for security-critical endpoints it is a hardening miss.

### 4.1 Policy applied in this session

Apply `#[serde(deny_unknown_fields)]` to admin / security-critical request bodies. Leave inference bodies untouched to preserve SDK compatibility.

### 4.2 Bodies hardened in this session (10 sites)

| File | Type | Route(s) | Risk if unknown fields accepted |
|:--|:--|:--|:--|
| `handlers/compliance.rs:191` | `ModuleToggle` | `PUT /v1/compliance/policies/{module}` | Silent typo (`enabled_at`) makes a no-op look like success |
| `handlers/compliance.rs:247` | `ApplyTemplateRequest` | `POST /v1/compliance/policies/template` | Same — typos drop silently |
| `handlers/compliance.rs:366` | `AuditQueryParams` | `GET /v1/compliance/audit` (query) | Typo in `tenat=` skips intended filter, leaks rows |
| `handlers/compliance.rs:553` | `GenerateReportRequest` | `POST /v1/compliance/reports/generate` | Same — silent loss of `tenant` scoping |
| `handlers/trust.rs:70` | `RevocationQuery` | `GET /v1/trust/revocation_status` | Defensive |
| `handlers/trust.rs:101` | `ExchangeTokenRequest` | `POST /v1/auth/exchange_token` | Silent drop of `scopes` shape would mint wrong-scope token |
| `handlers/models.rs:467` | `InstallRequest` | `POST /v1/models/install` | Silent drop of integrity-related field |
| `handlers/system.rs:285` | `AuditLogQuery` | `GET /v1/audit/log` (query) | Defensive |
| `types.rs:404` | `PowerTransitionRequest` | `POST /v1/power/transition` | Typed `action` is the only switch — defensive |
| `types.rs:602` | `UpdateDownloadRequest` | `POST /v1/updates/download` | Silent drop of `auto_install` typo could trigger unwanted install |

### 4.3 Bodies NOT hardened (and why)

| File | Type | Route(s) | Reason for leniency |
|:--|:--|:--|:--|
| `types.rs:17` | `ChatCompletionRequest` | `/v1/chat/completions`, `/v1/completions` | OpenAI-compat: SDKs ship `seed`, `response_format`, `n`, `user`, `logit_bias`, etc. — strict deny breaks every wrapper |
| `types.rs:66` | `EmbeddingRequest` | `/v1/embeddings` | Same |
| `types.rs:93` | `StructuredGenerationRequest` | `/v1/generate/structured` | Same (and `response_format.json_schema` is intentionally permissive — it carries arbitrary user schema) |
| `types.rs:124` | `FunctionCallRequest` | `/v1/generate/function_call` | Same — tool definitions are an open shape by spec |
| `types.rs:52`, `types.rs:145`, `types.rs:155` | `ApiChatMessage`, `ToolDefinition`, `FunctionDefinition` | nested inside above | Same surface; same rationale |

If a future tightening pass wants strict deny on inference bodies, it must land alongside a wire-compat note in `RC1-CHANGES.md` and a deprecation window for SDK callers — not in this session.

---

## 5. Fixes applied this session

### 5.1 Inference adapter-error discrimination (3 sites)

Bring `embeddings`, `structured_generation`, and `function_call` to the same `FrameworkError` discrimination as `chat_completions` (which already does this at `inference.rs:115-126`). Each fix is one `.map_err` block grown from a single-line `ApiError::InternalError` to a small `match` on `FrameworkError::ProcessCrashed` / `ResponseTimeout` / other.

### 5.2 SEC-012 strict bodies (10 sites)

Add `#[serde(deny_unknown_fields)]` above the ten structs enumerated in §4.2. No other change to those structs. Wire shape is unchanged for any well-formed client; mistyped admin requests now fail loudly with `MAI-1001 BadRequest` instead of silently being accepted.

### 5.3 What was NOT changed

- No new dependency added (no `tower-governor`, no `validator` crate, no `jsonschema`).
- No breaking change to the inference / OpenAI-compat surface.
- No change to the `ApiError` taxonomy itself — it already covers every observed code path.
- No code path was removed; this is hardening, not refactoring.

---

## 6. Verification

- `cargo check -p mai-api` clean (re-run on every fixed site).
- `cargo test -p mai-api` runs full lib + integration; no regression.
- `cargo clippy -p mai-api -- -D warnings` clean.
- Subagent verification (per workspace `CLAUDE.md`) over every touched file before staging.

## 7. Follow-ups out of scope for J-08

- A tighter wire contract for the OpenAI-compat surface (§4.3) — needs a deprecation window and SDK alignment.
- Per-route rate-limit overrides (`RateLimiter` is currently global per key) — would belong in a future hardening session, not under DOUGHERTY.
- A handler-level integration test suite that walks every error path end-to-end — partially covered by `mai-api/tests/*`, but no single doc enumerates which `MAI-XXXX` codes are exercised in CI. Worth a `J-XX` follow-up if the rescan still flags Error Handling < 75.
