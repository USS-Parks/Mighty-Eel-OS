# Demo Suite

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Acquirer technical reviewers, sales engineers, internal QA
**Status:** BF-7 scenarios (Appendix A §A.11). Absorbed by Session 46.
**Last Updated:** 2026-05-22 (post-S44+BF-6)

This file is the reproducible script catalog. Every scenario maps to
green tests; an acquirer should be able to run each one cold.

For the buyer narrative behind these demos, see
[`ACQUISITION-PACKAGE.md`](ACQUISITION-PACKAGE.md). For the
integration sequence, see [`BUYER-INTEGRATION-GUIDE.md`](BUYER-INTEGRATION-GUIDE.md).

---

## Headline scenario — Trust Manifold (BF-7 primary deliverable)

This is the eight-step Trust Manifold demo flow called out in plan
§A.11. It proves the entire stack: identity → claim → disconnect →
local inference → restricted request → enforcement → audit linkage →
degraded mode.

### Pre-flight

- Deployment profile: `mai/deployment/local-mai-node/`
- Reference scaffold: `apps/openbao-trust-demo/`
- Mock cloud bridge: the scaffold simulates the bridge locally until
  live OpenBao bring-up

### Eight steps

| # | Step | What it proves | Where it lives |
|---:|---|---|---|
| 1 | Authenticate through the OpenBao-backed bridge | The bridge mints a short-lived `TrustClaim` from an IdP identity | `simulate_bridge_authentication()` in `apps/openbao-trust-demo/main.py` |
| 2 | Issue short-lived Lamprey claim | Claim carries `tenant_id`, `subject_id`, `subject_hash`, `compliance_scopes`, `allowed_routes`, `trust_bundle_version` | `BridgeResult.claim` |
| 3 | Disconnect the cloud trust core | Local node continues operating on signed bundle | Step 3 calls `client.trust.bundle_status()`; the fallback path keeps the demo running even when the bridge is unreachable |
| 4 | Continue local inference using the valid signed bundle | `LocalTrustCache::record_signed_refresh` verifies ML-DSA-87 sig + canonical JSON + BLAKE3 before storing | `mai-compliance::trust_cache` |
| 5 | Submit a restricted request | A request with `compliance_scopes=["hipaa"]` arrives at the router | `apps/openbao-trust-demo/main.py:run_inference` |
| 6 | Lamprey enforces local-only route | Composer applies deny-wins / most-restrictive-route; OCAP and HIPAA gates fire | `mai-compliance/src/policy/composer.rs` |
| 7 | Audit log links credential event, policy decision, inference event | `CorrelationFields` chains `credential_event_id → lamprey_decision_id → mai_request_id` in §A.9 schema | `mai-compliance/src/audit/{entry,chain,store}.rs` |
| 8 | Expired bundle forces degraded or restricted mode | `LocalTrustCache` transitions through Connected → Degraded → Stale → Expired → Air-gapped; policy restricts route | `LocalTrustCache::connectivity_state()`; integration test `test_degraded_bundle_marks_signature_unverified` |

### Run it

```powershell
# Full pipeline against the BF-6 live endpoints:
pytest apps/openbao-trust-demo/tests/ -v

# Then exercise it interactively against a running mai-api:
$env:MAI_API_KEY = "im-..."
python apps/openbao-trust-demo/main.py
# Dry-run (no inference call):
python apps/openbao-trust-demo/main.py --dry-run
# Custom prompt:
python apps/openbao-trust-demo/main.py --prompt "Summarize my session policy."
```

### Expected audit-proof linkage

After step 7 the audit summary printed by the scaffold has the
following shape (the `correlation_id` is the join key into the
`AuditLog`):

```json
{
  "claim_id": "claim-<uuid>",
  "tenant_id": "im-demo",
  "subject_hash": "sha256:<32 hex>",
  "service_identity": "openbao-trust-bridge",
  "trust_bundle_version": "local-dev",
  "route_decision": "local_only",
  "correlation_id": "openbao-demo-claim-<uuid>",
  "bundle_state": "live",
  "bundle_connectivity": "connected",
  "bundle_signature_verified": true
}
```

A regulator (or the acquirer) can then query
`GET /v1/compliance/audit?correlation_id=openbao-demo-claim-<uuid>`
and re-verify the chain with `POST /v1/compliance/audit/verify`. The
chain verification is BLAKE3 link + ML-DSA-87 periodic signature; off-host
re-verification needs only the public verification key and the
canonical JSON.

### Acceptance criteria (§A.11)

- [x] Trust Manifold scenario runs end-to-end against BF-6 live endpoints
- [x] No prompt, completion, or embedding content crosses the cloud trust
      layer — the bridge mints claims, never reads payloads
- [x] Audit proof links identity, policy, route, and inference event
- [x] Expired bundle forces degraded mode (test
      `test_degraded_bundle_marks_signature_unverified`)
- [x] Scenario is reproducible by someone other than the original implementer
      (17 green tests in `apps/openbao-trust-demo/tests/`)

---

## Supporting scenarios

These rounds out the demo suite the plan calls for in §12.5 and
§A.11. Each is independently runnable.

### Healthcare scenario (HIPAA)

- **Goal:** Show PHI detection forcing a local-only route.
- **Scaffold:** `apps/compliance-routed/`
- **Trigger:** Send a chat containing a PHI marker (the scaffold uses
  fixture text that the HIPAA module flags).
- **Expected:** Route decision `local_only_allowed`, reason code
  `hipaa.phi_detected`, audit entry written with policy version
  recorded.
- **Run:** `pytest apps/compliance-routed/tests/`
- **Report:** Generate a HIPAA report via
  `client.compliance.reports.generate({"template": "HIPAA"})` — the
  output includes a `TrustSection` even though no live OpenBao is
  wired (the trust section reflects local-dev state).

### Defense scenario (ITAR / EAR)

- **Goal:** Show controlled-technical-data detection blocking unsafe
  backends.
- **Scaffold:** `apps/compliance-routed/`
- **Trigger:** Send a chat with ITAR-controlled markers (fixture).
- **Expected:** Route decision `deny`, reason code
  `itar.controlled_data`, audit entry includes
  `trust_bundle_version` and `service_identity`.
- **Report:** ITAR template renders a defense-events section.

### Tribal data sovereignty scenario (OCAP)

- **Goal:** Show consent and possession evaluation governing route.
- **Scaffold:** `apps/tribal-sovereignty/`
- **Trigger:** Submit a request with OCAP metadata indicating tribal
  health data without explicit cultural consent.
- **Expected:** The 9-stage OCAP pipeline halts at cultural consent;
  route is local-only or refused; audit records the governance
  reason.
- **Run:** `pytest apps/tribal-sovereignty/tests/`
- **Report:** OCAP template includes possession status, consent
  status, treaty consent — every field is auditable.

### Multi-domain conflict scenario

- **Goal:** Show rule precedence resolving a multi-policy hit.
- **Setup:** A request that triggers HIPAA + OCAP simultaneously.
- **Expected:** Composer applies `OCAP > ITAR > HIPAA` precedence;
  deny-wins; most-restrictive-route; explanation enumerates which
  policy contributed which reason code.
- **Verify:** `cargo test -p mai-compliance policy::composer`
  exercises the precedence matrix.

### Dashboard walkthrough

- **Goal:** Show that an operator can reach every state from one UI.
- **Component:** `mai/compliance-dashboard/` (FastAPI)
- **Pages:** Overview, Audit, Reports, Policy, Alerts, Health.
- **Run:**
  ```powershell
  $env:MAI_DASHBOARD_ADMIN_TOKEN = "dashboard-dev"
  uvicorn compliance-dashboard.app:app
  ```
- **Verify:** Trust panel shows `mode=connected`, bundle version,
  claim count, offline backlog. Audit page filters by tenant,
  module, decision. Reports page generates and downloads HIPAA /
  ITAR / OCAP outputs. Alerts page consumes the
  `/v1/compliance/feed` SSE stream live.

### Operator scenario (single-pane health)

- **Goal:** Show the operator/admin pane reading every health surface
  in one call.
- **Scaffold:** `apps/operator/`
- **Five panels:** models, scheduler, power, trust, system.
- **Trust panel:** Reads `/v1/trust/status` (BF-6 live); reports
  `mode={connected|degraded|stale_not_expired|expired|air-gapped}`,
  bundle version, claim count, offline backlog.
- **Run:** `pytest apps/operator/tests/` (12 green).

### Local secure inference scenario

- **Goal:** Show the minimal authenticated streaming chat path.
- **Scaffold:** `apps/local-secure-inference/`
- **Run:** `pytest apps/local-secure-inference/tests/` (6 green).

### RAG reference scenario

- **Goal:** Show ingest → embed → cosine retrieval → answer in one
  flow.
- **Scaffold:** `apps/rag-reference/`
- **Run:** `pytest apps/rag-reference/tests/` (6 green).

---

## Combined acquisition demo (§12.6 of the plan)

This is the full-platform story for an acquirer's executive
audience. Twelve steps, all reproducible:

1. Operator authenticates through the OpenBao-backed Trust Manifold
2. Bridge issues a short-lived Lamprey claim
3. Local MAI SDK exchanges the claim via
   `POST /v1/auth/exchange_token`
4. Lamprey classifies the request (HIPAA + OCAP fixtures)
5. Router consults trust context, policy bundle version, air-gap state
6. Scheduler places the request using topology + KV affinity
7. Local model returns inference output
8. Audit log writes an entry linking `credential_event_id →
   lamprey_decision_id → mai_request_id`
9. Operator queries the audit chain via the dashboard's Audit page
10. Compliance report generator emits a signed HIPAA report
11. Cloud trust core is disconnected; demo continues with local
    bundle
12. Bundle is expired manually; degraded mode kicks in; route
    decisions tighten

Steps 1-10 run today (all components landed through S44 + BF-6).
Steps 11 and 12 are demonstrated via the
`apps/openbao-trust-demo/tests/test_degraded_bundle_marks_signature_unverified`
path plus the `airgap-demo` deployment profile.

---

## Reproducibility checklist

A reviewer should be able to walk this in under 30 minutes:

- [ ] Clone the repo
- [ ] `cargo test -p mai-compliance --lib` → 326+ green
- [ ] `pytest apps/` (each scaffold separately) → 61+ green
- [ ] `pytest mai-api/tests/compliance_integration.py` → 17 green
- [ ] Start `mai-api` via `cargo run --bin mai-api`
- [ ] Run `python apps/openbao-trust-demo/main.py --dry-run` against it
- [ ] Confirm `GET /v1/trust/status` returns `{"mode": "connected", ...}`
- [ ] Open the dashboard, generate a HIPAA report, download it
- [ ] Verify the downloaded report's certification with
      `verify_certified_report` (helper in `mai-compliance::reports`)

If any step fails, the gap is documented in
[`KNOWN-ISSUES.md`](KNOWN-ISSUES.md).
