# PSPR-03 — WSF route inventory & auth-classification matrix (step 1)

Source: `crates/wsf-api/src/lib.rs` `router()` (lines 53–65) + handlers.
All privileged routes are currently **unauthenticated**: any network caller reaches them.

| Route | Handler | Privileged? | Action | Authority today | Must derive server-side | Rate-limit | Receipt today | Finding |
|:--|:--|:--:|:--|:--|:--|:--:|:--:|:--:|
| `POST /v1/tokens/issue` | `issue` (225) | **YES** | mint signed trust token | caller-authored `tenant_id`, `subject_id`, `roles`, `budget`, `allowed_models` | tenant, subject, service identity, roles, audience, budget ceiling | **required** | yes (bridge correlation) | **AF-01** (this prompt) |
| `POST /v1/tokens/attenuate` | `attenuate` (279) | **YES** | sign child token | caller supplies full `child` (oracle) | child derived from restrictions on a verified parent | required | **none** (gap) | AF-02 (PSPR-04) |
| `POST /v1/tokens/verify` | `verify` (256) | no | verify presented token sig/expiry | token in body; grants no authority | n/a (read-only verify) | advisable | n/a | — |
| `POST /v1/envelopes/seal` | `seal` (288) | **YES** | seal plaintext into envelope | token in body | tenant/owner/audience/label from principal+policy | required | yes | AF-04-adjacent (PSPR-08) |
| `POST /v1/envelopes/unseal` | `unseal` (311) | **YES** | unseal envelope | token in body only | tenant/subject/audience binding verified pre-decrypt | required | yes | **AF-04** (PSPR-08) |
| `POST /v1/credentials/exchange` | `exchange` (333) | **YES** | broker AWS creds | token + **caller-selected `role_arn`** | grant_id → role via server policy | required | (none here) | **AF-05** (PSPR-09) |
| `GET /v1/receipts` | `receipts` (361) | **YES** | read evidence ledger | **none**; arbitrary `field`/`value`, cross-tenant | authn principal + mandatory tenant predicate | required | n/a (is the reader) | **AF-14** (PSPR-16) |
| `GET /openapi.json` | `openapi` (370) | no | serve OpenAPI doc | none | n/a | n/a | n/a | — |
| `GET /healthz` | closure (63) | no | liveness | none | n/a | n/a | n/a | — |

## AF-01 root (this prompt)

`issue` (225–254) constructs `IssueTokenRequest::new(req.tenant_id, req.subject_id, req.roles)`
directly from the request body and signs it. No `WsfPrincipal`, no authentication middleware, no
tenant binding. Every authoritative field is attacker-controlled.

## Required shape (server-derived authority)

1. A trusted `WsfPrincipal` (mTLS/workload identity or another reviewed production authenticator),
   extracted by middleware applied to every **YES** route above; missing/invalid identity → fail
   closed (401/403) before any signing/ledger/broker call.
2. `issue`: tenant, subject-namespace, service identity, roles, audience, and budget **ceiling**
   come from the principal's server-side policy. The request body becomes a *narrowing* request:
   requested models must be a subset of policy; requested budget ≤ ceiling; requested roles ⊆
   granted. Anything wider → reject before signing.
3. Separate permissions for self / delegated / service / administrative issuance.
4. Per-principal issuance rate limit; receipt every allow **and** deny without token material.
5. OpenAPI/SDK versioned; legacy unauthenticated behavior disabled by default (fail-closed).

## Adversarial matrix to freeze (from PSPR-03 VERIFY + §4 corpus)

anonymous · forged/expired/wrong-key identity · wrong-audience · wrong-tenant · role-elevation ·
model-widening · budget-widening — each must fail **before** `bridge.issue_token`. Plus a
route-conformance test that fails CI when a privileged route lacks policy metadata, and a
two-tenant black-box issuance run against live OpenBao (PSPR-28).

## Status

Step 1 (this inventory) complete. Steps 2–8 (the `WsfPrincipal` extractor, authenticated middleware,
server-derived authority, permission separation, rate limiting + deny receipts, OpenAPI/SDK
versioning, adversarial suite, live OpenBao proof) are the core implementation — a large,
multi-file change owed. AF-01 remains **OPEN**.
