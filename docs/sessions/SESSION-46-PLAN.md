# Session 46 — Compliance Demo Suite + Integration Testing (Plan)

> **STATUS — CLOSED (2026-05-23)**
> S46 shipped (commit `22f0f66`). Gate D closed. 9 mai-compliance integration tests + perf baselines landed; composer P99 1.5 µs / audit 9003/s / report 16.7 ms — measured headroom against all targets. Kept as historical record; no further S46 work.

**Status:** Plan (work in progress) — **CLOSED 2026-05-23**
**Date:** 2026-05-23
**Phase:** L (Compliance Governance)
**Closes:** Gate D — Acquisition-Ready Release
**Depends on:** S45 acquisition documentation package (`1753af4`)
**Blocks:** Nothing (final mainline session)

This document is the working plan for Session 46. It maps the
roster's S46 prompt (`MAI-BUILD-PROMPT-ROSTER-v2.md` §3761) and the
plan's §1371 acceptance criteria onto a concrete file layout,
test inventory, and gate-D evidence checklist. It will be deleted
or archived once `READY.md` lands and S46 commits.

---

## 1. Scope summary

The Lamprey compliance stack (S36–S45) is fully built and
documented. S46 turns the four demo scripts under
`docs/acquisition/demos/` into runnable, deterministic, signed-off
integration tests, plus the surrounding evidence the buyer needs
to verify the stack works end-to-end without trusting Island
Mountain source code.

In scope:

- 6 end-to-end scenario tests (HIPAA, ITAR, OCAP, multi-domain,
  audit-tamper, Trust Manifold disconnected/expired).
- 1 performance baseline test (composer P99, audit throughput,
  report generation) — gated by env var so CI stays fast.
- 1 production-readiness deliverable (`docs/acquisition/READY.md`)
  enumerating evidence with measured numbers.
- Updates to `docs/INDEX.md` and `docs/SESSION-LOG.md`.

Out of scope (deferred or already covered):

- A dedicated `tests/compliance/` top-level crate. We extend the
  existing `mai-api/tests/` integration harness because it already
  wires `AppState` (PolicyManager + AuditLog + ReportManager +
  LocalTrustCache + verifier). A new crate would duplicate that
  setup for no real benefit.
- Dashboard browser walkthrough. The dashboard test suite already
  has 20 green page tests; the new `test_dashboard_load` from the
  prompt collapses into asserting the SDK's
  `client.compliance.status()` returns the expected shape, which is
  already exercised by `mai-sdk-python/tests/test_compliance.py`.
- Vault-AEAD sealed audit-store integration. The audit store
  already takes a `StoreSealer` via builder, and BF-3 ML-DSA
  signers prove the contract. Wiring the live vault is a separate
  hardening session.
- S31 roster "Part 2" scaffolds. Gate B was closed by S30 per the
  plan; the plan-vs-roster scope split is documented in
  [`SESSION-LOG.md`](SESSION-LOG.md).

---

## 2. File layout

```
mai-compliance/tests/
  compliance_demos.rs            (NEW — 6 scenario tests + shared DemoEnv)
  compliance_perf.rs             (NEW — perf baseline, env-gated)

docs/acquisition/
  READY.md                       (NEW — production readiness)

docs/
  INDEX.md                       (EDIT — link new tests + READY)
  SESSION-LOG.md                 (EDIT — append S46 completion)
```

No new crates. No new public APIs. No changes to `mai-compliance`
source — S46 is exclusively a tests + docs session. This keeps
the surface that auditors review unchanged from the S45 snapshot.

Scenario tests live in `mai-compliance/tests/` rather than
`mai-api/tests/` because they exercise compliance-engine semantics
end-to-end (composer + audit + reports). The HTTP surface around
those engines is already proven by the 17 tests in
`mai-api/tests/compliance_integration.rs` — duplicating that
plumbing for the scenarios would slow compile time and obscure
what's actually being asserted.

---

## 3. Shared test harness — `DemoEnv`

A single helper struct in `mai-compliance/tests/compliance_demos.rs`
constructs a deterministic environment for every scenario:

```rust
struct DemoEnv {
    template: PolicyTemplate,
    composer: PolicyComposer,
    audit: AuditLog,
    reports: ReportManager,
    policy_version: String,
    request_clock: AtomicU64,  // monotonic nanos for AuditRecordInput
}

impl DemoEnv {
    fn with_template(template: PolicyTemplate) -> Self { ... }
    fn next_timestamp(&self) -> u64 { ... }
    fn record(&self, bundle: &PolicyBundle, agg: &AggregateDecision) -> AuditEntry { ... }
    fn certify(&self, kind: ReportType, scope: ReportScope) -> CertifiedReport { ... }
}
```

The harness uses only public `mai-compliance` APIs — no new
constructors required. `PolicyTemplate::{Standard, Healthcare,
Defense, TribalGovernment}` selects which modules fire; the
composer and ReportManager are wired straight through.

The harness uses `TrustContext::for_local_dev()` plus per-scenario
mutations (revocation, offline mode, expired bundle) — no fake
trust types, no shim layers.

---

## 4. Scenario test inventory

### 4.1 `test_hipaa_workflow`

| Step | API call | Assertion |
|---|---|---|
| Build PHI input | `PhiDetector::baseline().scan(...)` | hits include `MedicalRecordNumber`, `PatientName` |
| Run HIPAA | `BaaEnforcer::evaluate(...)` then `ModuleDecision::from_hipaa` | `allowed = false`, `route = Local` |
| Compose | `PolicyComposer::compose([hipaa])` | `aggregate.route_selected = Local`, reason contains `hipaa.phi.*` |
| Record | `audit.record(...)` | entry has `module = Hipaa`, `decision = LocalOnlyAllowed` |
| Report | `reports.generate_certified(HipaaAuditTrail, ...)` | certified report verifies via `verify_certified_report` + `TrustSection` non-empty |

Replays the [healthcare.md](acquisition/demos/healthcare.md) demo
deterministically.

### 4.2 `test_itar_workflow`

- ITAR USML Category I keyword + non-US `ActorContext`.
- `JurisdictionEvaluator::evaluate(...)` → `DenyExport`.
- Composer yields `route_selected = Local`, `allowed = false`.
- Audit entry has `Itar` module and `Deny` decision.
- `ItarComplianceSummary` report generated, certified, verifies.

Replays [defense.md](acquisition/demos/defense.md).

### 4.3 `test_ocap_workflow`

- Tribal identifier + `ConsentStatus::Withheld` + sacred-role
  required.
- `OcapEvaluator::evaluate(...)` → `RouteLocal` with
  `OcapReason::ConsentRequired`.
- Composer yields local-only-allowed.
- `OcapGovernanceReport` includes `tribal_data_detected = true`,
  certified, verifies.

Replays [tribal.md](acquisition/demos/tribal.md).

### 4.4 `test_multi_domain`

- Input contains PHI + ITAR keyword + tribal identifier.
- All three modules fire.
- Composer asserts:
  - `aggregate.allowed = false` (any-deny-wins from ITAR)
  - `aggregate.route = Local` (most-restrictive)
  - reasons list contains entries from all three modules
  - `aggregate.modules_applied` = `[Hipaa, Itar, Ocap]` in
    canonical order
- Audit entry retains all three `modules_applied`.
- `MonthlyComplianceDigest` rolls up the cross-module decision.

Replays [multi-domain.md](acquisition/demos/multi-domain.md).
This is the precedence test — proves OCAP > ITAR > HIPAA per
`PolicyTemplate::TribalGovernment::priority()`.

### 4.5 `test_audit_tamper`

- Record three decisions.
- Pull the second entry, mutate `routing_reason`, write it back
  via a `StoreSealer` test double that exposes raw vec mutation.
- Call `verify_chain` — expect `ChainError::LinkBreak { at: 2 }`.
- Call `audit.record_chain_break("test mutation")` — expect
  `Severity::Critical` escalation.

This is the only test that needs a small test-only helper on the
audit store. The helper lives inside the test file, not in
`mai-compliance` — keeps the production crate clean.

### 4.6 `test_trust_manifold_disconnected_and_expired`

Two subtests in one test fn:

1. **Disconnected:** flip `TrustContext.offline_mode = true`,
   record a decision, generate a report, assert
   `TrustSection.offline_intervals` is non-empty and the report's
   `mode_summary` says `degraded`.
2. **Expired bundle:** set `revocation_status = Unknown`,
   `trust_bundle_version` older than current — expect ITAR
   evaluator returns the configured `trust.revocation_unknown_for_itar`
   rule and the composer enforces local-only.

Covers plan §A.14 acceptance criteria: disconnected operation and
expired-trust-bundle scenarios. The `openbao-trust-demo` scaffold
already exercises the SDK path; this test pins the server-side
contract.

---

## 5. Performance baseline — `mai-compliance/tests/compliance_perf.rs`

Gated by `RUN_PERF_TESTS=1` so CI stays under a minute. When the
gate is set, the test measures:

| Metric | Target (from S46 acceptance) | Method |
|---|---|---|
| Composer P50 / P95 / P99 | P99 < 5 ms @ 100 req/s | 5 000 mixed bundles, sorted latencies |
| Audit append throughput | ≥ 1 000 entries/s sustained | 10 000 entries, measure wall-time |
| Report generation (30-day) | < 10 s | populate 1 000 entries, run `generate_certified(HipaaAuditTrail, ...)` |

The test prints a table to stdout in the format the `READY.md`
checklist will quote. Numbers measured on the implementer's
machine go straight into `READY.md` with the hardware footnote.

PHI detection already has its own perf test
(`mai-compliance/tests/phi_perf.rs`) which proves <10 ms — that's
referenced from `READY.md`, not duplicated.

---

## 6. `docs/acquisition/READY.md` outline

```
# Acquisition Production Readiness

## 1. Test evidence
   - Rust workspace lib tests: 1196 green
   - mai-api integration: 17 + 6 new = 23 green
   - mai-compliance perf (gated): composer P99 = ?, append = ?/s
   - Python SDK: 94 green; dashboard: 20 green; scaffolds: 61 green
   - Total: 1394+ runnable, 0 known failing

## 2. Demo evidence
   - Healthcare (HIPAA): automated, green — test_hipaa_workflow
   - Defense (ITAR/EAR): automated, green — test_itar_workflow
   - Tribal (OCAP): automated, green — test_ocap_workflow
   - Multi-domain: automated, green — test_multi_domain
   - Trust Manifold disconnected/expired: automated, green

## 3. Performance against S46 targets
   - Composer P99: <target> vs 5 ms target → PASS/observed
   - Audit append: <target> vs 1000/s target → PASS/observed
   - Report generation: <target> vs 10 s target → PASS/observed
   - PHI detection P99: covered by phi_perf.rs (<10 ms)

## 4. Documentation evidence
   - Acquisition package: 13 docs from S45 + this READY
   - INDEX.md cross-links all S45 + S46 artifacts
   - Buyer integration guide present
   - Demo suite documented + automated

## 5. Known issues (honest)
   - Vault-AEAD sealed audit-store: contract proven, live wiring deferred
   - Dashboard browser walkthrough: SDK + page tests only; manual UI
     review still required before customer-facing deployment
   - S31 roster scaffolds (MedRecord/HomeBase/Estate AI): out of plan
     scope; deliberately not built

## 6. Compliance certification statement
   (signed-off statement that the implementation matches the
   plan's stated guarantees)
```

---

## 7. Gate D evidence checklist

Per `BUILD-EXECUTION-PLAN-V2-UPDATED.md` §1434:

- [ ] healthcare demo runs end to end — `test_hipaa_workflow`
- [ ] defense demo runs end to end — `test_itar_workflow`
- [ ] tribal data sovereignty demo runs end to end — `test_ocap_workflow`
- [ ] multi-policy conflict demo runs end to end — `test_multi_domain`
- [ ] Trust Manifold demo runs end to end —
  `test_trust_manifold_disconnected_and_expired` + existing
  `openbao-trust-demo` scaffold
- [ ] compliance decisions are explainable — assertions on
  `aggregate.reasons` cover this in every scenario
- [ ] trust claims are verifiable — `TrustSection` non-empty +
  `verify_certified_report` succeeds in every scenario
- [ ] audit logs verify — `verify_chain` called in every test
- [ ] reports generate — every scenario generates a certified report
- [ ] dashboard works — existing 20 dashboard tests + SDK
  `client.compliance.*` cover this
- [ ] SDK covers compliance APIs — covered by S44 SDK additions
- [ ] acquisition docs are complete — S45 + READY.md
- [ ] known issues are current and honest — `READY.md` §5

---

## 8. Anti-truncation discipline

All new files >40 lines go through `$env:TEMP\opencode\` staging
per the workspace integrity protocol:

- `tail -5` + `wc -c` after each stage.
- Independent subagent integrity pass before commit for any batch
  of 3+ files.
- `mai/.integrity/scripts/verify-tree.sh` on the full change list.
- Individual `git add <path>` — never `git add -A`.
- `git diff --cached --stat` inspected before commit.

---

## 9. Sequencing

1. ✅ Draft this plan (THIS DOCUMENT)
2. Scaffold `DemoEnv` in `compliance_demos.rs` + first test
   (HIPAA) to validate the harness shape.
3. ITAR + OCAP scenarios (parallel module setups).
4. Multi-domain (depends on 2 + 3).
5. Audit tamper detection.
6. Trust Manifold disconnected/expired.
7. Perf baseline.
8. Run full suite, capture numbers.
9. Write `READY.md` with measured numbers.
10. Update `INDEX.md` + `SESSION-LOG.md`.
11. Verify-tree + staged commit.

Each step is an independent task in the S46 task list and lands
its own intermediate verification.
