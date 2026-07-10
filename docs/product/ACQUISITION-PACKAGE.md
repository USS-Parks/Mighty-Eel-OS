# Acquisition Package \-- Five Defensible Points

**Project:** Island Mountain Model Abstraction Interface (MAI) **Audience:** Acquirer technical and product diligence teams **Status:** BF-7 narrative (Appendix A Section A.11). Absorbed by Session 45 acquisition documentation. **Last Updated:** 2026-05-22 (post-S44+BF-6)

---

## What this document is

Every claim in this package maps to landed code, passing tests, and documented contracts in this repository. Nothing here is a roadmap item or a design intention. If a claim cannot be verified by running a command or reading a specific module, it does not appear below.

Island Mountain AI enables regulated organizations to run AI locally, prove who accessed it, govern what data can be processed, enforce where inference runs, and verify the audit trail afterward.

The five defensible points that follow are the technical proof of that sentence.

---

## Point 1 \-- Hardware-aware local inference scheduling

**Claim:** MAI's scheduler is not a queue. It is a multi-factor placement engine that reasons about GPU topology, KV cache residency, batching opportunity, power state, and instance health on every request.

**Why it is defensible:**

- Topology graph derived from `nvidia-smi`, NVLink/PCIe edge weights, and CPU affinity groups (`mai-scheduler/src/topology/`, 41 unit tests).  
- KV cache reuse is a first-class placement input \-- warm-cache routing is preferred over cold even when the cold instance is less loaded (`mai-scheduler/src/kv/`, 53 unit tests; continuation affinity in `placement.rs`).  
- Continuous batching with admission control and preemption hierarchy (System \> High \> Normal \> Background), tested under starvation pressure (`mai-scheduler/src/batch/`, 52 tests).  
- Cross-instance balancer with net-benefit migration scoring and soft eviction across hot, warm, and cold KV tiers (`kv/offload.rs`, `kv/tiered.rs`, `preemption.rs`, `balancer.rs`).  
- Decision cache keyed on `(model_alias, priority, load_bucket)` with hit/miss counters (`decision_cache.rs`).  
- Production trace replay harness compares policies deterministically at `(trace, seed, policy)` and emits acquisition-ready Markdown and JSON reports (`tools/simulator/replay_compare.py`, `tools/simulator/report.py`).

**Why competitors cannot copy this quickly:** every shipping API wrapper or model gateway treats placement as round-robin or simple load-watermark. The scheduler in MAI is the only piece that survives a hardware refresh \-- when TetraMem MX100 lands in 2028, the HIL contract absorbs it without changing the policy layer (see `docs/HANDOFF.md` Section "The HIL is the moat").

---

## Point 2 \-- OpenBao-backed enterprise trust with local verification and offline bundles

**Claim:** Enterprise identity, secrets, PKI, signing, revocation, and audit-device functions sit in a separate trust plane (OpenBao). The local MAI/Lamprey appliance verifies short-lived claims against signed bundles without round-tripping to the cloud, so rural and air-gapped sites keep operating when the trust core is unreachable.

**Why it is defensible:**

- Three-ring trust manifold documented and implemented: Cloud OpenBao Core feeds the Lamprey Trust Bridge, which maintains the Local Trust Cache (`docs/TRUST-MANIFOLD.md`, `docs/OPENBAO-INTEGRATION.md`).  
- Service identity model with per-service OpenBao policies \-- no broad shared token in the target design (`docs/SERVICE-IDENTITY.md`, `mai-compliance::trust::ServiceIdentity`).  
- Signed claim and signed policy bundle verification using ML-DSA-87 over canonical-JSON with BLAKE3 (`mai-compliance::bundle`, `docs/TRUST-BUNDLE-SPEC.md`). Invalid signatures and expired bundles are rejected with cache state preserved.  
- Local trust cache with explicit connectivity states \-- connected, degraded, stale-not-expired, expired, and air-gapped \-- and the policy layer restricts route or refuses inference when material is stale (`mai-compliance::trust_cache::LocalTrustCache`, `docs/LOCAL-TRUST-CACHE.md`).  
- Live trust endpoints in `mai-api`:  
  - `GET /v1/trust/status` (consolidated mode)  
  - `GET /v1/trust/claims` (admin)  
  - `GET /v1/trust/bundle_status`  
  - `GET /v1/trust/revocation_status?claim_id=...`  
  - `POST /v1/auth/exchange_token` (profile-selected `TrustExchangeMode`: production mode forwards to the acquirer's OpenBao bridge, the local-dev synthetic exchange exists only under an explicitly-permissive dev profile; wire shape identical in every mode, no handler code is edited)  
- Python SDK trust and auth namespaces wired and tested (`client.trust.*`, `client.auth.exchange_token`); 94 SDK tests plus 17 `mai-api` integration tests cover the full surface.  
- Five deployment postures shipped: `deployment/local-dev`, `deployment/cloud-trust-core`, `deployment/local-mai-node`, `deployment/airgap-demo`, and `deployment/ship` (the customer-installable production posture, enforced by the production guard) \-- each carrying a `profile.toml` selecting trust mode, compliance template, air-gap state, and cloud-route permission.

**Hard rule, enforced architecturally:** prompt, completion, embedding, PHI, ITAR/EAR-controlled, and OCAP-governed payloads do **not** move through the cloud trust system. The Trust Manifold moves identity, claims, signatures, revocation snapshots, and audit correlation IDs only. This separation is verifiable by reading the route handlers and the `mai-compliance::bundle` signing payload.

---

## Point 3 \-- Compliance routing across HIPAA, ITAR/EAR, and OCAP

**Claim:** Three sovereign policy engines share a normalized decision shape and a deny-wins composer. Every decision carries reason codes, trust context, and an audit-log entry \-- there are no silent allows.

**Why it is defensible:**

- HIPAA engine with PHI detection, minimum-necessary reason codes, and role-gated local-only restrictions (`mai-compliance/src/hipaa/`).  
    
- ITAR/EAR engine with controlled-technical-data indicators, jurisdiction-aware backend eligibility, and trust-claim access-class checks (`mai-compliance/src/itar.rs`, `ear.rs`, `jurisdiction.rs`).  
    
- OCAP engine with a 9-stage decision pipeline (`mai-compliance/src/ocap/`):  
    
  1. Scope check  
  2. Revocation check  
  3. Trust local-only ceiling  
  4. Possession evaluation  
  5. Control evaluation  
  6. Sacred role gate  
  7. Elder role gate  
  8. Cultural consent gate  
  9. Treaty consent gate \-- then route-local or allow


- Policy runtime that normalizes `RequestMetadata`, `TrustContext`, `ConnectivityState`, `PolicyBundleVersion`, and `ClassificationResult` into one composer call. Deny-wins, most-restrictive-route, with explicit precedence OCAP over ITAR over HIPAA (`mai-compliance/src/policy/composer.rs`).  
    
- TTL-bounded decision cache keyed on stable inputs, ignoring `request_id` and timestamp so identical decisions do not re-evaluate the rule set (`mai-compliance/src/policy/cache.rs`).  
    
- Four policy templates ship: Standard, Healthcare, Defense, TribalGovernment (`mai-compliance/src/policy/templates.rs`).  
    
- Test coverage: 1196 Rust workspace lib tests; `mai-compliance` alone at 326+ tests; 17 `mai-api` integration tests cover the HTTP surface.

**Why this is acquisition-grade:** every shipping compliance product in the AI space classifies after the fact. MAI classifies at placement time and the route decision is the audit record. An acquirer can demonstrate this to a regulator in one screen \-- the compliance dashboard at `/v1/compliance/feed` shows decisions, route codes, and policy version live.

---

## Point 4 \-- Tribal data sovereignty (OCAP) as a rare differentiator

**Claim:** OCAP is not implemented as keyword matching. It is implemented as governance metadata, possession evaluation, consent status, and tribal-source trust evaluation \-- the same shape recognized by First Nations Information Governance Centre OCAP(R) doctrine.

**Why it is defensible:**

- The 9-stage decision pipeline encodes the OCAP principles explicitly (`mai-compliance/src/ocap/mod.rs`, `tribal_data.rs`, `treaty.rs`, `cultural.rs`, `ocap_rules.rs`).  
- Every `OcapDecision` carries `claim_id`, `tenant_id`, `subject_hash`, `trust_bundle_version`, `service_identity`, `offline_mode`, and `revocation_status` \-- full trust context for audit correlation.  
- Treaty consent and cultural consent are distinct gates with separate reason codes; sacred role and elder role have priority paths.  
- Missing scope refuses with `OcapError::ScopeMissing` rather than defaulting to allow \-- fail-closed by design.  
- `TribalGovernment` policy template ships out of the box.  
- Tribal Sovereignty reference scaffold (`apps/tribal-sovereignty/`, 9 tests) demonstrates local-only enforcement with explicit `SovereigntyViolation` errors when route or model guards trip.

**Why this matters to a buyer:** every other AI infrastructure vendor's compliance story stops at HIPAA. OCAP is a procurement unlock for tribal health systems, tribal energy and resource agencies, and provincial and state governments operating on tribal land. It is also a defensible reputational asset \-- Island Mountain treats tribal data sovereignty as a first-class architectural concern, not a checkbox.

---

## Point 5 \-- Physical air-gap enforcement tied to inference routing and tamper-evident audit records

**Claim:** Air-gap is a routing input, not a deployment flag. When a node is air-gapped, the router refuses cloud routes; when a request demands a cloud route under air-gap, the audit log records a hash-chained refusal with policy version and credential correlation.

**Why it is defensible:**

- Canonical `ConnectivityState` (connected, degraded, air\_gapped) consumed by the router and the policy composer (`mai-core/src/airgap/`).  
- Loopback and wildcard bind enforcement at the API layer prevents accidental binding to external interfaces.  
- `/v1/system/airgap` exposes the operator-visible state; the compliance dashboard surfaces it on the trust panel.  
- Tamper-evident audit log: append-only BLAKE3 hash chain with optional ML-DSA-87 periodic signatures (`mai-compliance/src/audit/entry.rs`, `chain.rs`, `store.rs`). `AuditLog::verify_full` detects link breaks, non-monotonic IDs, and invalid periodic signatures.  
- Audit correlation links each credential event to its Lamprey decision and the MAI request: `credential_event_id`, `lamprey_decision_id`, `mai_request_id` \-- per the Section A.9 schema.  
- Compliance reports include a `TrustSection` on every output: credential validation summary, trust bundle version history, revocation snapshot mix, offline-interval reconstruction, service-identity events, policy-version history, and audit verification status.  
- Report certification: ML-DSA-87 over canonical-JSON rendering, so the signed audit proof is format-independent and verifiable off-host (`mai-compliance/src/reports/pdf.rs`).

**Why this is the moat:** an acquirer can hand a regulator the audit chain, the signed report, and the live verification tool \-- and the regulator can re-verify off-host without trusting MAI source code. That property does not exist in any cloud AI product.

---

## How an acquirer can verify each point in under an hour

| Point | Verification path | Expected output |
| :---- | :---- | :---- |
| 1 \-- Scheduler | `cargo test -p mai-scheduler --lib` then `python tools/simulator/replay_compare.py --trace examples/sample-trace.ndjson` | 324+ green tests; Markdown report ranking policies by placement score |
| 2 \-- Trust Manifold | `pytest apps/openbao-trust-demo/tests/` then `curl /v1/trust/status` against `deployment/local-dev` | 17 green tests; live `{"mode": "connected", "bundle_version": ...}` JSON |
| 3 \-- Compliance routing | `cargo test -p mai-compliance --lib` then `pytest mai-api/tests/compliance_integration.py` | 326+ green tests; 17 HTTP integration tests green |
| 4 \-- OCAP | `pytest apps/tribal-sovereignty/tests/` then read `mai-compliance/src/ocap/` | 9 green tests; 9-stage pipeline visible in source |
| 5 \-- Air-gap \+ audit | `cargo test -p mai-compliance audit` then `verify_certified_report` against a generated report | Hash chain verification clean; ML-DSA-87 signature valid; `verify_full` returns no link breaks |

---

## What is intentionally not yet in the box

These are documented gaps, not surprises:

- Live OpenBao deployment. The contract, schemas, verifier, cache, correlation, and local-dev token stub all ship. The acquirer plugs an OpenBao instance in through the ship profile's `[openbao]` section \-- production mode selects the OpenBao bridge exchange (`TrustExchangeMode`) with no handler edit, and the wire shape is unchanged.  
- Production HTTPS transport for the OTA update client. Core download logic is transport-agnostic; the acquirer wires their preferred CDN.  
- Hardware-dependent burn-in (Scout/Ranger boot timings, 72-hour stability). Documented in `docs/KNOWN-ISSUES.md` Issue \#8; `scripts/burn-in.sh` emits the deferred-criteria list per run.

---

## Acquisition-thesis one-pager

OpenBao        \= enterprise trust, secrets, PKI, crypto backbone

MAI            \= local inference \+ hardware-aware scheduling platform

Lamprey        \= compliance routing, policy engine, audit proof layer

Trust Manifold \= bridge between enterprise trust and local regulated inference

Together: regulated organizations can run AI locally, prove access,

govern data, enforce location, and verify the audit trail.

See [`BUYER-INTEGRATION-GUIDE.md`](http://BUYER-INTEGRATION-GUIDE.md) for the integration sequence and [`DEMO-SUITE.md`](http://DEMO-SUITE.md) for the reproducible demo scenarios.  
