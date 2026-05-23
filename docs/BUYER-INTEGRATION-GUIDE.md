# Buyer Integration Guide

**Project:** Island Mountain Model Abstraction Interface (MAI) **Audience:** Acquirer integration engineers, security architects, SRE leads **Status:** BF-7 narrative (Appendix A Section A.11) **Last Updated:** 2026-05-22 (post-S44+BF-6)

This guide explains how an acquirer wires their existing OpenBao deployment, IdP, and SRE tooling into MAI's trust and compliance layer without disturbing the local-first guarantee.

For the buyer-facing narrative, see [`ACQUISITION-PACKAGE.md`](http://ACQUISITION-PACKAGE.md). For reproducible demo scripts, see [`DEMO-SUITE.md`](http://DEMO-SUITE.md).

---

## 30-minute security architect review path

A security architect evaluating this integration does not need to run the full demo suite. The eight questions below have specific answers in specific source files. Read them in order; each answer should close one line of inquiry before the next opens.

| \# | Question | Where to look | What you are confirming |
| ----: | :---- | :---- | :---- |
| 1 | Does any prompt, completion, or regulated payload cross the cloud trust boundary? | `mai-compliance::bundle::canonical_bytes` | Signing payload contains only identity and policy metadata \-- no content |
| 2 | Do the live trust endpoints return metadata only, never content? | `mai-api/src/handlers/trust.rs` | Each handler returns claim metadata or bundle state; no inference payload touches these routes |
| 3 | Does air-gap state actually block cloud routes at the router level? | `mai-core/src/airgap/` and the router | `ConnectivityState::AirGapped` is a first-class router input; cloud routes are refused, not just discouraged |
| 4 | Does the policy composer fail closed \-- deny on ambiguity rather than allow? | `mai-compliance/src/policy/composer.rs` | Deny-wins, most-restrictive-route; no silent allows in any code path |
| 5 | Does OCAP fail closed on missing governance scope? | `mai-compliance/src/ocap/mod.rs` | `OcapError::ScopeMissing` refuses the request; the pipeline never defaults to allow |
| 6 | Do audit correlation events queue locally when the SIEM sink is unreachable? | `mai-compliance/src/audit/store.rs` | `AuditStore` holds 4096 events with a drop counter; drains when connectivity returns |
| 7 | Does signed bundle verification reject expired and tenant-mismatched material? | `mai-compliance::trust_cache::record_signed_refresh` | Verification rejects invalid, expired, and mismatched bundles; cache state is preserved on failure |
| 8 | Is the signed compliance report verifiable off-host without trusting MAI source code? | `mai-compliance/src/reports/pdf.rs` | ML-DSA-87 over canonical-JSON; verification needs only the public key and the canonical JSON |

Every item maps to a specific module. None requires running a command or standing up infrastructure \-- a code review suffices. The full-platform demo in [`DEMO-SUITE.md`](http://DEMO-SUITE.md) makes the same guarantees observable at runtime.

---

## The trust boundary \-- what crosses and what does not

The cleanest way to understand MAI's integration shape is to be explicit about which bytes move where.

### Crosses the boundary (cloud OpenBao \<-\> local MAI/Lamprey)

- Identity metadata (subject ID, tenant ID, role names)  
- Short-lived Lamprey claims (JSON; see schema below)  
- Signatures and HMAC subject hashes  
- mTLS service certificates  
- Revocation snapshots (metadata only)  
- Signed policy bundles  
- Audit correlation IDs (`credential_event_id`)  
- Tenant policy metadata (version strings, template names)

### Does NOT cross the boundary by default

- Prompts and completions  
- Embeddings  
- PHI content  
- ITAR/EAR-controlled technical content  
- OCAP-governed tribal data  
- Any regulated payload

This separation is enforced architecturally. The Trust Manifold's signing payload contract (`mai-compliance::bundle`) explicitly excludes content; the only data fed into a signature is identity and policy metadata. Verifiable by code review.

---

## What is swappable and what stays fixed

A security architect's second question after the boundary check is usually: "what does the acquirer actually have to touch, and what guarantees survive that touch?" The answer has four surfaces.

### Handler body swap: `POST /v1/auth/exchange_token`

MAI ships a local-dev token stub at this endpoint. In production, the acquirer swaps the handler body to call their own OpenBao-backed Trust Bridge. **The wire shape \-- request JSON, response JSON, error codes \-- does not change.** No client code moves. No SDK methods change. The SDK's `client.auth.exchange_token()` call is identical before and after the swap.

The bridge is the only component that converts an enterprise IdP identity into a Lamprey claim. Everything downstream of the endpoint \-- claim verification, trust cache storage, policy evaluation, audit correlation \-- is unaffected by which bridge implementation sits behind the handler.

### Deployment profile swap: `mai/deployment/`

The four shipped profiles (`local-dev`, `cloud-trust-core`, `local-mai-node`, `airgap-demo`) are `profile.toml` files. Switching postures is a config change, not a code change. The profile selects trust mode, compliance template, air-gap state, and cloud-route permission. No API surface changes between profiles.

See the deployment postures table below for the full matrix.

### Compliance template swap: `mai-compliance/config/templates/`

The four built-in templates (Standard, Healthcare, Defense, TribalGovernment) are `PolicyBundle` definitions. An acquirer extends or replaces them by adding a tenant config under `mai-compliance/config/templates/` and selecting it via `compliance.template`. The policy runtime, composer, and audit log are unaffected \-- the same decision shape flows through regardless of which template is active.

### OTA transport swap

The model update manifest client is transport-agnostic. The acquirer wires their preferred CDN or distribution channel at the transport layer; the manifest comparison, differential shard planning, and resumable download logic are unchanged.

---

## The Lamprey claim schema

A claim is the unit of trust an acquirer's OpenBao Trust Bridge mints and hands to the local MAI/Lamprey node. The full shape:

{

  "claim\_id": "claim\_abc",

  "tenant": "tribal-health-demo",

  "subject": "user:12345",

  "roles": \["care\_coordinator"\],

  "compliance\_scopes": \["hipaa", "ocap"\],

  "allowed\_routes": \["local\_only"\],

  "allowed\_models": \["lamprey/fast", "lamprey/medical-local"\],

  "max\_data\_classification": "restricted",

  "emergency\_access": false,

  "policy\_version": "2026.05.22-tribal-health-v4",

  "trust\_bundle\_version": "2026.05.22.001",

  "expires\_at": "2026-05-22T12:15:00Z"

}

The acquirer's bridge controls every field. The local node verifies the signature, checks expiry, and feeds the claim into the policy runtime as `TrustContext`.

Full spec: [`TRUST-MANIFOLD.md`](http://TRUST-MANIFOLD.md), [`TRUST-BUNDLE-SPEC.md`](http://TRUST-BUNDLE-SPEC.md).

---

## Deployment postures (`mai/deployment/`)

The four shipped profiles cover the realistic acquirer postures:

| Profile | Trust mode | Compliance template | Air-gap | Cloud route |
| :---- | :---- | :---- | :---- | :---- |
| `local-dev` | local-dev stub | Standard | off | enabled (dev only) |
| `cloud-trust-core` | live OpenBao client | Standard / template-per-tenant | off | enabled |
| `local-mai-node` | local cache \+ periodic bundle refresh | template-per-tenant | optional | conditional on claim |
| `airgap-demo` | local cache only | Defense | on | refused |

Each profile is a `profile.toml` plus a `README.md`. Switching profiles is a config change; no code moves.

---

## Integration sequence

The order matters \-- earlier steps wire the trust floor, later steps exercise it.

### Step 1 \-- Provision OpenBao service identities

For each of these workloads, create a Kubernetes service account and an OpenBao policy with least-privilege paths:

- `mai-api`  
- `mai-scheduler`  
- `mai-adapter-manager`  
- `lamprey-router`  
- `lamprey-policy`  
- `lamprey-audit`  
- `lamprey-dashboard`  
- `local-trust-cache`  
- `audit-correlation-service`

Path conventions are documented in [`SERVICE-IDENTITY.md`](http://SERVICE-IDENTITY.md). No service should rely on a shared broad token.

### Step 2 \-- Wire your IdP into the Lamprey Trust Bridge

The bridge is the only component that converts an enterprise IdP identity into a Lamprey claim. It signs claims with a Transit key held in OpenBao. Replace MAI's local-dev token stub by swapping the body of `POST /v1/auth/exchange_token` to call your bridge. The wire shape is unchanged, so no client code moves. See "What is swappable and what stays fixed" above for the full guarantee.

### Step 3 \-- Configure the local trust cache

On each local MAI/Lamprey node, set the `[trust]` block of the deployment profile to point at your bridge's public verification key. The cache will:

- Verify every claim it stores  
- Refuse unsigned, invalid, expired, or tenant-mismatched bundles  
- Preserve cache state on verification failure (no silent overwrite)  
- Surface mode via `GET /v1/trust/status` to the operator dashboard

Spec: [`LOCAL-TRUST-CACHE.md`](http://LOCAL-TRUST-CACHE.md).

### Step 4 \-- Choose a compliance template per tenant

Templates wire the HIPAA, ITAR/EAR, and OCAP modules with sensible defaults. The four built-ins are Standard, Healthcare, Defense, and TribalGovernment. Each is a `PolicyBundle` you can extend or replace without touching the policy runtime.

Apply a template via the dashboard:

PUT /v1/compliance/policies/template

Content-Type: application/json

{"template": "Healthcare", "version": "1.0"}

Or by setting `compliance.template` in the deployment profile at startup.

### Step 5 \-- Pipe audit correlation to your SIEM

The audit chain stays local. What ships to a SIEM is the metadata side of `CorrelationFields`:

{

  "credential\_event\_id": "cred\_evt\_123",

  "lamprey\_decision\_id": "dec\_456",

  "mai\_request\_id": "req\_789",

  "tenant": "tribal-health-demo",

  "subject\_hash": "hmac:...",

  "service\_identity": "lamprey-router",

  "policy\_version": "2026.05.22.001",

  "trust\_bundle\_version": "2026.05.22.001",

  "decision": "local\_only\_allowed"

}

No prompt, completion, or embedding crosses. The offline queue in `AuditStore` holds 4096 events with a drop counter when the SIEM endpoint is unreachable, then drains when connectivity returns. See [`AUDIT-CORRELATION.md`](http://AUDIT-CORRELATION.md).

### Step 6 \-- Stand up the dashboard

The FastAPI dashboard at `compliance-dashboard/` exposes the live operator surface: trust mode, bundle freshness, audit chain verification, policy decisions, alerts. Gate the admin token via `MAI_DASHBOARD_ADMIN_TOKEN`. The dashboard is the only buyer-facing UI; everything else is API-driven.

### Step 7 \-- Verify the integration

Run `pytest apps/openbao-trust-demo/tests/` to confirm the local trust loop is healthy. Then walk the [`DEMO-SUITE.md`](http://DEMO-SUITE.md) Trust Manifold scenario end to end, including the disconnect step. If the chain prints `bundle_signature_verified: true` after disconnecting the cloud trust core, the integration is correct.

---

## SDK touchpoints

The Python SDK is the supported integration surface. All methods listed below remain unchanged across the handler-body and profile swaps described above.

| Namespace | Methods | What it does |
| :---- | :---- | :---- |
| `client.trust` | `status`, `claims`, `bundle_status`, `revocation_status` | Reads the local trust cache. Metadata only \-- no content crosses. |
| `client.auth` | `exchange_token` | Mints a short-lived session token. Handler body swaps to your bridge in production; this call is identical before and after. |
| `client.compliance` | Policy: `status`, `policies`, `policies/{module}`, `policies/reload`, `policies/template`, `modules/{name}/enable`, `modules/{name}/disable` / Audit: `audit`, `audit/{id}`, `audit/verify`, `audit/integrity` / Reports: `reports`, `reports/generate`, `reports/{id}`, `reports/{id}/download`, `reports/{id}` (DELETE) | Full policy, audit, and reports surface. |
| `client.scheduler` | `metrics`, `instance/{id}`, `anomalies`, etc. | Read-only scheduler observability. |
| `client.models`, `client.chat`, `client.embed`, `client.stream_chat` | Inference. Trust-context-aware; the SDK forwards the active claim where the deployment is wired. |  |

SDK errors map to HTTP status codes: `AuthenticationError`, `PermissionError`, `RateLimitError`, `ClaimExpiredError`, `TrustCacheStaleError`, `AirGapViolationError`, `PowerStateUnavailableError`, `MaiError` (base). See `mai-sdk-python/docs/error-handling.md`.

---

## Boundary contract review checklist

Use this during the pre-acquisition security architecture review. Each item has a specific source location; none requires running code.

- [ ] Confirm `mai-compliance::bundle::canonical_bytes` excludes any prompt or completion content. Trace the signing payload.  
- [ ] Confirm the live trust endpoints return metadata only. Read `mai-api/src/handlers/trust.rs`.  
- [ ] Confirm air-gap state blocks cloud routes. Read `mai-core/src/airgap/` and the router.  
- [ ] Confirm audit correlation events queue locally when the SIEM sink is unreachable. Read `mai-compliance/src/audit/store.rs`.  
- [ ] Confirm the policy composer is fail-closed (deny-wins). Read `mai-compliance/src/policy/composer.rs`.  
- [ ] Confirm OCAP refuses on missing scope rather than allowing. Read `mai-compliance/src/ocap/mod.rs`.  
- [ ] Confirm signed bundle verification rejects expired and tenant-mismatched material. Read `mai-compliance::trust_cache::record_signed_refresh`.  
- [ ] Confirm report certification uses canonical-JSON ML-DSA-87. Read `mai-compliance/src/reports/pdf.rs`.

All eight are verifiable from source. None relies on marketing material or vendor claims.

---

## What an acquirer brings to the integration

- An OpenBao deployment (or a target to deploy one into)  
- An IdP that can drive the Trust Bridge (Okta, Azure AD, Auth0, workload identity \-- any source the bridge can consume)  
- A SIEM or audit-correlation sink for the metadata-only stream  
- A CDN or distribution channel for OTA model updates (transport layer; the manifest client is transport-agnostic)  
- Hardware for the Scout, Ranger, and Pack Leader tiers per `configs/*.toml`

What an acquirer does not need to bring:

- A compliance classifier (HIPAA, ITAR, and OCAP modules ship)  
- A trust bundle signer (ML-DSA-87 ships)  
- A scheduler (it is the moat)  
- A local trust cache (ships)  
- An audit chain (ships)  
- A dashboard (ships)

