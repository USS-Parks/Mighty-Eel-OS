# Demo Suite

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Acquirer technical reviewers, sales engineers, internal QA
**Status:** BF-7 scenarios (Appendix A Section A.11). Absorbed by Session 46.
**Last Updated:** 2026-05-22 (post-S44+BF-6)

This is the reproducible demo catalog for MAI's local-first inference,
Lamprey governance, and audit-proof workflow. Each scenario is written
as a proof: what the reviewer should see, what it demonstrates, and
where the backing tests or code live.

For the buyer narrative behind these demos, see
[`ACQUISITION-PACKAGE.md`](ACQUISITION-PACKAGE.md). For the
integration sequence, see [`BUYER-INTEGRATION-GUIDE.md`](BUYER-INTEGRATION-GUIDE.md).

---

## Reviewer Path

Three personas, three paths. Pick the one that matches who is in the room.

**Technical due diligence reviewer (25 min):** Run the Trust Manifold
tests, execute the dry-run against `mai-api`, open the dashboard, generate
one signed compliance report. That path proves identity, policy, routing,
local inference, audit correlation, and report verification without
requiring a live cloud trust deployment. It maps directly to the five
defensible points in `ACQUISITION-PACKAGE.md`.

**Security architect (45 min):** Add the HIPAA, ITAR/EAR, OCAP, and
multi-domain conflict scenarios. Each is independently runnable and maps
to green tests. The goal is to confirm the policy composer is
fail-closed, that no regulated payload crosses the trust boundary, and
that the audit chain is tamper-evident.

**Executive audience (15 min, live):** Walk the Combined Acquisition Demo
below. Skip the scaffold tests. Show steps 1 through 10 live, then pull
the cloud trust core offline and show step 11. The point that lands:
inference continues, policy holds, and the audit chain is intact -- all
without the cloud.

---

## Headline Scenario: Trust Manifold

The Trust Manifold is the primary end-to-end demo. It proves the full
chain:

```text
identity -> signed claim -> local trust cache -> restricted request
         -> policy enforcement -> local route -> audit linkage
         -> degraded-mode behavior
```

The claim this scenario makes to a skeptical reviewer: MAI's data
sovereignty guarantee is not a configuration flag. It is a cryptographically
enforced, auditable, offline-capable property that a regulator can
verify off-host using only the public key and the canonical JSON.

### What To Have Ready

- Deployment profile: `mai/deployment/local-mai-node/`
- Reference scaffold: `apps/openbao-trust-demo/`
- A running `mai-api` when exercising the interactive path
- Mock cloud bridge: the scaffold mints the claim locally until live
  OpenBao bring-up; the local trust and auth endpoints are live BF-6
  surfaces

### Eight Proof Moments

| # | Step | What it proves | Where it lives |
|---:|---|---|---|
| 1 | Authenticate through the OpenBao-backed bridge | The bridge mints a short-lived `TrustClaim` from an IdP identity | `simulate_bridge_authentication()` in `apps/openbao-trust-demo/main.py` |
| 2 | Issue short-lived Lamprey claim | Claim carries `tenant_id`, `subject_id`, `subject_hash`, `compliance_scopes`, `allowed_routes`, `trust_bundle_version` | `BridgeResult.claim` |
| 3 | Disconnect the cloud trust core | Local node continues operating on its signed bundle; no inference interruption | Step 3 calls `client.trust.bundle_status()`; the fallback path keeps the demo running even when the bridge is unreachable |
| 4 | Continue local inference on the valid signed bundle | `LocalTrustCache::record_signed_refresh` verifies ML-DSA-87 signature, canonical JSON, and BLAKE3 before storing | `mai-compliance::trust_cache` |
| 5 | Submit a restricted request | A request with `compliance_scopes=["hipaa"]` arrives at the router | `apps/openbao-trust-demo/main.py:run_inference` |
| 6 | Lamprey enforces local-only route | Composer applies deny-wins and most-restrictive-route; OCAP and HIPAA gates fire | `mai-compliance/src/policy/composer.rs` |
| 7 | Audit log links credential event, policy decision, and inference event | `CorrelationFields` chains `credential_event_id`, `lamprey_decision_id`, and `mai_request_id` per the Section A.9 schema | `mai-compliance/src/audit/entry.rs`, `chain.rs`, `store.rs` |
| 8 | Expired bundle forces degraded or restricted mode | `LocalTrustCache` transitions through Connected, Degraded, Stale, Expired, and Air-gapped; policy restricts route at each boundary | `LocalTrustCache::connectivity_state()`; integration test `test_degraded_bundle_marks_signature_unverified` |

**What the reviewer sees after step 7:** a `correlation_id` that joins
three distinct events -- credential issuance, policy decision, and
inference execution -- into one auditable chain. Ask them to query
the chain live. That is the moment that differentiates MAI from every
logging-after-the-fact compliance product.

**What the reviewer sees after step 8:** inference stops accepting cloud
routes, not because a flag was flipped, but because the trust material
expired and the policy composer re-evaluated the route decision. The
audit log records the refusal with the expired bundle version and the
credential correlation ID intact.

### Run The Proof

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

### What Success Looks Like

The demo prints an audit-ready summary. The `correlation_id` is the
join key into the `AuditLog`; it is the thread that connects the
credential event, Lamprey decision, and MAI request.

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

With that `correlation_id` in hand, the reviewer can query the full
audit chain and re-verify it independently:

```bash
# Pull the correlated audit chain:
GET /v1/compliance/audit?correlation_id=openbao-demo-claim-<uuid>

# Re-verify the chain (needs only public key + canonical JSON):
POST /v1/compliance/audit/verify
```

Chain verification is BLAKE3 link integrity plus ML-DSA-87 periodic
signatures. Off-host re-verification requires only the public
verification key and the canonical JSON -- no MAI source code, no
trust in the vendor.

### Acceptance Criteria

- [x] Trust Manifold scenario runs end-to-end against BF-6 live endpoints
- [x] No prompt, completion, or embedding content crosses the cloud trust
      layer -- the bridge mints claims, never reads payloads
- [x] Audit proof links identity, policy, route, and inference event
- [x] Expired bundle forces degraded mode (test
      `test_degraded_bundle_marks_signature_unverified`)
- [x] Scenario is reproducible by someone other than the original implementer
      (17 green tests in `apps/openbao-trust-demo/tests/`)

---

## Supporting Scenarios

Each scenario below is independently runnable and targets a specific
policy family or operator surface. Run them in order when doing a full
security architecture review; run any one of them in isolation when the
audience has a specific compliance question.

### Healthcare: HIPAA

**What this proves:** PHI detection is pre-inference, not post-hoc. The
route decision is the audit record. A regulator watching the dashboard
sees the PHI flag, the local-only route, and the policy version -- in
one screen, before the model generates a single token.

- **Scaffold:** `apps/compliance-routed/`
- **Trigger:** Send a chat containing a PHI marker. The scaffold uses
  fixture text that the HIPAA module flags deterministically.
- **Expected output:** Route decision `local_only_allowed`, reason code
  `hipaa.phi_detected`, audit entry written with policy version recorded.
- **Run:**
  ```bash
  pytest apps/compliance-routed/tests/ -v
  ```
- **Generate the compliance report:**
  ```python
  client.compliance.reports.generate({"template": "HIPAA"})
  ```
  The output includes a `TrustSection` even without a live OpenBao
  deployment; the trust section reflects local-dev state and is
  auditable.
- **What the reviewer says:** "So the route decision happens before
  the request reaches the model, and the decision itself is the audit
  entry." Correct. That is the entire compliance argument.

### Defense: ITAR / EAR

**What this proves:** Controlled technical data is blocked at the
routing layer, not filtered from a response after the fact. No
export-controlled content reaches an ineligible backend.

- **Scaffold:** `apps/compliance-routed/`
- **Trigger:** Send a chat with ITAR-controlled markers (fixture).
- **Expected output:** Route decision `deny`, reason code
  `itar.controlled_data`, audit entry includes `trust_bundle_version`
  and `service_identity`.
- **Run:**
  ```bash
  pytest apps/compliance-routed/tests/ -v
  ```
- **Generate the defense report:**
  ```python
  client.compliance.reports.generate({"template": "ITAR"})
  ```
  The ITAR template renders a defense-events section with every
  controlled-data hit and its corresponding policy decision.
- **What the reviewer says:** "The deny happens before inference,
  and the audit entry shows which policy version made that call."
  Correct. If the policy is updated and the bundle version changes,
  that change is auditable in the same chain.

### Tribal Data Sovereignty: OCAP

**What this proves:** OCAP governance is not keyword matching. It is
a nine-stage pipeline that evaluates consent, possession, and
sovereignty metadata before routing. Missing consent halts the
pipeline and refuses the request -- it does not default to allow.

- **Scaffold:** `apps/tribal-sovereignty/`
- **Trigger:** Submit a request with OCAP metadata indicating tribal
  health data without explicit cultural consent.
- **Expected output:** The 9-stage OCAP pipeline halts at cultural
  consent. Route is local-only or refused. Audit entry records the
  governance reason, the stage that halted, and the full trust context.
- **Run:**
  ```bash
  pytest apps/tribal-sovereignty/tests/ -v
  ```
- **Generate the sovereignty report:**
  ```python
  client.compliance.reports.generate({"template": "OCAP"})
  ```
  The OCAP template includes possession status, consent status, and
  treaty consent -- every field is independently auditable.
- **What the reviewer says:** "Every other vendor's compliance story
  stops at HIPAA." That observation is the market position. OCAP is
  the procurement unlock for tribal health systems, tribal energy and
  resource agencies, and governments operating on tribal land.

### Multi-Domain Conflict

**What this proves:** When a request triggers multiple policy engines
simultaneously, the composer resolves the conflict deterministically
and explains which policy contributed which reason code. There are no
silent allows and no undefined precedence outcomes.

- **Setup:** A request that triggers HIPAA and OCAP simultaneously.
- **Expected output:** Composer applies OCAP, then ITAR, then HIPAA
  precedence. Deny-wins. Most-restrictive-route. The decision payload
  enumerates every contributing policy and its reason code.
- **Verify the precedence matrix:**
  ```bash
  cargo test -p mai-compliance policy::composer -- --nocapture
  ```
- **Verify the HTTP surface:**
  ```bash
  pytest mai-api/tests/compliance_integration.py -v -k "multi_domain"
  ```
- **What the reviewer says:** "So if OCAP and HIPAA disagree, OCAP
  wins, and the audit record shows both." Correct. The composer does
  not hide the conflict -- it records every policy that touched the
  decision.

### Dashboard Walkthrough

**What this proves:** Every trust state, policy decision, audit
entry, and compliance report is visible to an operator in one UI,
without writing code.

- **Component:** `compliance-dashboard/` (FastAPI)
- **Pages:** Overview, Audit, Reports, Policy, Alerts, Health
- **Start the dashboard:**
  ```powershell
  $env:MAI_DASHBOARD_ADMIN_TOKEN = "dashboard-dev"
  uvicorn compliance-dashboard.app:app
  ```
- **Verification sequence:**
  1. Trust panel: confirm `mode=connected`, bundle version, claim
     count, and offline backlog are present.
  2. Audit page: filter by tenant, by policy module, by route
     decision. Confirm entries link back to `correlation_id`.
  3. Reports page: generate a HIPAA report, then an ITAR report.
     Download both. Confirm each carries a `TrustSection`.
  4. Alerts page: confirm the page is consuming the
     `/v1/compliance/feed` SSE stream live (events appear in
     real time as requests route through Lamprey).
  5. Verify the HIPAA report's certification:
     ```python
     from mai_compliance.reports import verify_certified_report
     verify_certified_report("hipaa-report.json")
     ```
- **What the reviewer says:** "This is the screen I hand to a
  regulator." That is the correct framing. The dashboard is the
  operator-facing proof surface; everything behind it is API-driven
  and independently verifiable.

### Operator Health

**What this proves:** Every health surface -- models, scheduler,
power, trust, and system -- is readable in a single operator call.
The trust panel exposes the full connectivity state machine so an
operator knows exactly where the node stands before taking action.

- **Scaffold:** `apps/operator/`
- **Five panels:** models, scheduler, power, trust, system
- **Trust panel reads:** `GET /v1/trust/status` (BF-6 live); reports
  `mode` as one of `connected`, `degraded`, `stale_not_expired`,
  `expired`, or `air-gapped`, plus bundle version, claim count, and
  offline backlog
- **Run:**
  ```bash
  pytest apps/operator/tests/ -v
  ```
  Expect 12 green tests.
- **Spot-check the trust endpoint directly:**
  ```bash
  curl -H "X-IM-Auth-Token: $MAI_API_KEY" \
       http://localhost:8420/v1/trust/status
  ```
  Expected: `{"mode": "connected", "bundle_version": "...", ...}`

### Local Secure Inference

**What this proves:** The minimal authenticated streaming chat path
works end-to-end. No compliance scaffolding required. This is the
baseline that every other scenario builds on top of.

- **Scaffold:** `apps/local-secure-inference/`
- **Run:**
  ```bash
  pytest apps/local-secure-inference/tests/ -v
  ```
  Expect 6 green tests.

### RAG Reference

**What this proves:** Ingest, embed, cosine retrieval, and grounded
answer generation work in one flow against the local inference stack.
No external vector database required.

- **Scaffold:** `apps/rag-reference/`
- **Run:**
  ```bash
  pytest apps/rag-reference/tests/ -v
  ```
  Expect 6 green tests.

---

## Combined Acquisition Demo

**Audience:** Acquirer executive and technical decision-makers in the
same room. **Estimated runtime:** 15 to 20 minutes live. Steps 1
through 10 run against landed code (S44 + BF-6). Steps 11 and 12 are
demonstrated via the degraded-bundle test path and the `airgap-demo`
deployment profile.

This is the "why MAI is different" sequence: identity stays in the
trust plane, data stays local, policy decides before inference runs,
and the audit trail can be verified afterward by someone who does not
trust the vendor.

1. Operator authenticates through the OpenBao-backed Trust Manifold
2. Bridge issues a short-lived Lamprey claim
3. Local MAI SDK exchanges the claim via `POST /v1/auth/exchange_token`
4. Lamprey classifies the request (HIPAA + OCAP fixtures)
5. Router consults trust context, policy bundle version, and air-gap state
6. Scheduler places the request using GPU topology and KV affinity
7. Local model returns inference output
8. Audit log writes an entry linking `credential_event_id`,
   `lamprey_decision_id`, and `mai_request_id`
9. Operator queries the audit chain via the dashboard's Audit page
10. Compliance report generator emits a signed HIPAA report; reviewer
    runs `verify_certified_report` off-host against the public key
11. Cloud trust core is disconnected; demo continues with local bundle;
    trust panel transitions to `degraded`
12. Bundle expires manually; route decisions tighten; the audit log
    records the refusal with the expired bundle version intact

**After step 10:** hand the reviewer the signed report and the public
key. Ask them to run `verify_certified_report` on their own machine.
When they confirm the signature is valid, the demo is done. Everything
that follows is detail.

**After step 12:** the question the room asks is "what happens to
inflight requests during degradation?" The answer is in the trust
cache connectivity state machine: requests that were already routed
complete, new requests are evaluated against the restricted policy
surface, and the audit log records every transition. Nothing is silent.

**Steps 11 and 12 are demonstrated via:**
```bash
# Degraded bundle behavior:
pytest apps/openbao-trust-demo/tests/test_degraded_bundle_marks_signature_unverified -v

# Air-gap posture:
# Switch to the airgap-demo profile and restart mai-api.
# Trust panel will show mode=air-gapped; cloud routes are refused.
```

---

## Reproducibility Checklist

A reviewer who has never seen this codebase should be able to walk
the full path in under 30 minutes. Each step below has a deterministic
expected output. If a step fails, the gap is documented in
[`KNOWN-ISSUES.md`](../KNOWN-ISSUES.md).

- [ ] Clone the repo
- [ ] `cargo test -p mai-compliance --lib` -- expect 326+ green
- [ ] `pytest apps/local-secure-inference/tests/` -- expect 6 green
- [ ] `pytest apps/rag-reference/tests/` -- expect 6 green
- [ ] `pytest apps/compliance-routed/tests/` -- expect green
- [ ] `pytest apps/tribal-sovereignty/tests/` -- expect 9 green
- [ ] `pytest apps/operator/tests/` -- expect 12 green
- [ ] `pytest apps/openbao-trust-demo/tests/` -- expect 17 green
- [ ] `pytest mai-api/tests/compliance_integration.py` -- expect 17 green
- [ ] Start `mai-api`: `cargo run --bin mai-api`
- [ ] `python apps/openbao-trust-demo/main.py --dry-run` -- expect
      audit summary JSON with `bundle_signature_verified: true`
- [ ] `curl http://localhost:8420/v1/trust/status` -- expect
      `{"mode": "connected", ...}`
- [ ] Open the dashboard, generate a HIPAA report, download it
- [ ] Run `verify_certified_report` against the downloaded report --
      expect signature verification clean

**If the dry-run fails:** check that `MAI_API_KEY` is set and that
`mai-api` is running on port 8420. The scaffold prints the specific
error; cross-reference with `KNOWN-ISSUES.md`.

**If a test suite fails:** each scaffold's `tests/` directory is
independently isolated. Run them one at a time, not with a top-level
`pytest apps/`. Module name collisions across scaffold test packages
cause collection failures when invoked together.
