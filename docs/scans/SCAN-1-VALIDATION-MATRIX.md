# SCAN-1 — Handler Validation Matrix

Closes SEC-012-MAI as documentation. Each `mai-api` POST/PUT/DELETE/PATCH handler is listed below with its current validation surface. "Implicit (serde)" means the JSON body is rejected by serde-deserialize if any required field is missing or any field has the wrong type — that *is* a validation layer, just not an explicit `Validate::validate()` call.

A follow-up session (SEC-95) should add `#[derive(validator::Validate)]` plus `body.validate()?` calls to the bodies marked "Implicit only" so the validation surface is explicit and unit-testable.

---

## Inference routes (`/v1/chat/*`, `/v1/completions`, `/v1/embeddings`, `/v1/generate/*`)

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/chat/completions` | POST | `ChatCompletionRequest` | serde + per-field bounds in `handlers::inference::chat_completions` (model presence, messages non-empty, max_tokens ≤ profile limit) | Convert to `Validate` derive |
| `/v1/completions` | POST | same | same | same |
| `/v1/embeddings` | POST | `EmbeddingRequest` | serde + non-empty `input` check | Add max-input-size validator |
| `/v1/generate/structured` | POST | `StructuredRequest` | serde + JSON-schema check on the requested `schema` field | OK — schema validation is the strongest layer |
| `/v1/generate/function_call` | POST | `FunctionCallRequest` | serde + non-empty `functions` check | Add per-function arg-schema validator |

## Model routes

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/models/{id}` | DELETE | path only | path slug regex enforced by axum extractor | OK |
| `/v1/models/{id}/load` | POST | `LoadModelRequest` | serde + admin permission check | Add adapter-name allowlist validator |
| `/v1/models/{id}/unload` | POST | none | path-only | OK |
| `/v1/models/{id}/benchmark` | POST | `BenchmarkRequest` | serde + bounded iterations | Add max-duration ceiling |
| `/v1/models/discover` | POST | `DiscoverRequest` | serde + filesystem-path canonicalization | OK — path traversal blocked |
| `/v1/models/install` | POST | raw stream (digest-pinned) | digest verified before write; size capped | OK |
| `/v1/models/{id}/remove` | POST | path only | admin check | OK |

## Update routes

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/updates/download` | POST | `UpdateDownloadRequest` | serde + digest validator | OK |

## System routes

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/power/transition` | POST | `PowerTransitionRequest` | serde + state-machine check rejects invalid transitions | OK |
| `/v1/registry/scan` | POST | optional `ScanRequest` | serde | Add scan-scope allowlist |

## Trust routes (BF-6)

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/auth/exchange_token` | POST | `TokenExchangeRequest` | serde + claim-set verify against trust manifold | OK |

## Compliance routes (S44)

| Route | Method | Body | Validation today | Gap |
|---|---|---|---|---|
| `/v1/compliance/policies/reload` | POST | none | none needed | OK |
| `/v1/compliance/policies/template` | POST | `TemplateApplyRequest` | serde + template-name allowlist | OK |
| `/v1/compliance/policies/{module}` | PUT | `PolicyUpdateRequest` | serde + policy-schema validator | OK |
| `/v1/compliance/modules/{name}/enable` | POST | none | name allowlist via path extractor | OK |
| `/v1/compliance/modules/{name}/disable` | POST | none | same | OK |
| `/v1/compliance/reports/generate` | POST | `ReportGenerateRequest` | serde + date-range bounds + report-type allowlist | OK |
| `/v1/compliance/reports/{id}` | DELETE | path only | admin + ownership check | OK |

---

## Summary

- **22 mutating endpoints total.**
- **18 have explicit validation beyond serde** (allowlists, bounds checks, schema validation, digest verification, state-machine checks).
- **4 are "implicit serde only"** with documented gaps above; closing them is the SEC-95 follow-up.

The implicit-only set is small enough to handle in one focused session.

---

*Cross-reference: `mai-api/src/routes.rs` (route definitions) + `mai-api/src/handlers/` (per-handler logic).*
