# Lamprey Compliance Governance Brief

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Acquirer compliance architects, regulated-industry CTOs,
governance/risk reviewers
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

Lamprey is the compliance-governance layer that sits **above** MAI's
inference engine. It is the acquisition IP. This brief explains what
it is, why it is structured the way it is, and how it differs from
every existing compliance/guardrail product in the AI infrastructure
space.

For positioning, see [`ACQUISITION-PACKAGE.md`](ACQUISITION-PACKAGE.md).
For deep architecture, see
[`acquisition/ARCHITECTURE.md`](acquisition/ARCHITECTURE.md). For
competitive context, see
[`acquisition/COMPETITIVE.md`](acquisition/COMPETITIVE.md).

---

## What Lamprey is

Lamprey is a three-layer stack that turns regulatory text into
inference-time decisions:

```
+-------------------------------------------------------+
|  Layer 3 — Audit                                       |
|  hash-chained audit log + reports + dashboard          |
|  (mai-compliance/src/audit, reports/, mai-api S44)     |
+-------------------------------------------------------+
|  Layer 2 — Policy                                      |
|  HIPAA + ITAR/EAR + OCAP modules + composer runtime    |
|  (mai-compliance/src/{hipaa,itar,ear,ocap}, policy/)   |
+-------------------------------------------------------+
|  Layer 1 — Router                                      |
|  query routing, sensitivity classifier, entity detect  |
|  (Lamprey router; binds to MAI scheduler placement)    |
+-------------------------------------------------------+
                       below sits MAI
                  inference + scheduler + HIL
```

The decision flow is **router → policy → audit**, then placement.
Crucially, the router decides local-vs-cloud *before* the scheduler
selects an instance, so a request that would violate ITAR or HIPAA
never reaches the placement code.

---

## Layer 1 — Router

The router is the layer that classifies an incoming request against
the policy bundle the operator has configured. It produces three
outputs:

1. **Classification** — what regulatory domains apply (HIPAA, ITAR/EAR,
   OCAP, none).
2. **Sensitivity** — how restricted the content is on a coarse scale
   that the composer can normalise.
3. **Entity detection** — what PHI, controlled technical data, or
   OCAP-governed metadata is present.

The classifier produces evidence, not assertions. The HIPAA module
decides what `phi_detected: true` means for *its* rules; the ITAR
module decides what `controlled_tech_data: true` means for *its*
rules. This separation lets the operator extend one module without
re-validating the others.

The router consumes `TrustContext` from BF-1/BF-2 — every routing
decision carries the originating subject, tenant, claim ID, allowed
routes, and offline-mode flag.

---

## Layer 2 — Policy

Three sovereign modules + a composer runtime.

### HIPAA module
Source: `mai-compliance/src/hipaa/`, Session 38.

- PHI detection across 18 HIPAA identifiers (deid module:
  `mai-compliance/src/deid.rs`, `phi.rs`, `medical_entities.rs`).
- Minimum-necessary reason codes for every decision.
- Role-gated local-only restrictions (a care coordinator gets
  different scope than a billing analyst).
- BAA (Business Associate Agreement) enforcement plumbed via
  `baa.rs`.
- `Healthcare` policy template ships out of the box.

### ITAR/EAR module
Source: `mai-compliance/src/{itar,ear,jurisdiction,tech_data}.rs`,
Session 39.

- Controlled-technical-data indicators (TAA classification, USML
  category hints).
- Jurisdiction-aware backend eligibility — backends are tagged with
  their physical location, and a request marked for ITAR can only
  route to backends whose `jurisdiction` is `US` and whose `air_gap`
  is `enabled` or whose `tenant_us_only` is `true`.
- Trust-claim access-class checks: a subject without an `itar_ear`
  compliance scope cannot consume an ITAR-marked request.
- `Defense` policy template ships out of the box.

### OCAP module (the rare one)
Source: `mai-compliance/src/ocap/`, Session 40.

OCAP — Ownership, Control, Access, Possession — is the First Nations
Information Governance Centre doctrine for tribal data sovereignty.
No competing product implements it; we did, with a nine-stage
decision pipeline:

```
1. scope check        — subject has 'ocap' compliance_scope?
2. revocation         — claim revoked?
3. trust local-only   — bundle requires local-only routing?
4. possession         — tribal authority possesses the data?
5. control            — authorised governance profile?
6. sacred role        — subject has sacred-role permission?
7. elder role         — subject has elder-role permission?
8. cultural consent   — explicit consent for cultural data?
9. treaty consent     — treaty-level consent for cross-border use?
```

Each stage produces a typed `OcapError` (with reason code) or
proceeds to the next; a missing scope refuses with
`OcapError::ScopeMissing` rather than defaulting to allow.
`TribalGovernment` policy template ships out of the box.

### Policy composer
Source: `mai-compliance/src/policy/`, Session 41.

The composer is the conflict-resolution layer. It normalises module
decisions into a shared `ModuleDecision` shape (allow / local-only /
deny + reason codes + compliance flags), then folds them with three
rules:

- **deny-wins** — any module that denies forces a deny outcome.
- **most-restrictive-route** — if one module accepts local-only and
  another accepts any-route, the result is local-only.
- **flag accumulation** — all reason codes are carried into the
  aggregate decision so the audit log records every contributor.

Precedence chain: **OCAP > ITAR > HIPAA** (used only when two modules
contribute conflicting reasons of equal strength). This precedence is
not a value judgement; it reflects that OCAP refusals are the hardest
to remediate downstream (data sovereignty cannot be repaired by
masking), ITAR is hard (legal exposure beyond compliance), and HIPAA
is well-tooled for remediation.

A TTL-bounded decision cache (default 60s, 1024-entry soft cap) keys
on stable inputs (tenant, source, model, classification, trust) and
ignores request_id and timestamp — identical decisions don't
re-evaluate the rule set.

### Policy templates
Source: `mai-compliance/src/policy/templates.rs`.

Four built-ins:

| Template | Intended deployment | Modules enabled |
|---|---|---|
| `Standard` | Local-dev, low-regulation | HIPAA off, ITAR off, OCAP off (templates only) |
| `Healthcare` | Hospital, clinic, MSO | HIPAA on with care-team roles, OCAP off |
| `Defense` | Defence contractor, dual-use research | ITAR on with US jurisdiction lock, EAR on |
| `TribalGovernment` | Tribal health, tribal energy, treaty land | OCAP on with sovereignty defaults, HIPAA on |

Templates are `PolicyBundle` values an operator can extend or
replace. Custom templates land in `mai-compliance/config/templates/`
and are selected via the `compliance.template` deployment key.

---

## Layer 3 — Audit

Source: `mai-compliance/src/audit/`, Session 42, with BF-5 correlation
overlay.

Every routing decision yields an `AuditEntry` with:

- A monotonically-increasing `id`.
- A previous-link BLAKE3 hash (`previous_hash`).
- A canonical-bytes BLAKE3 hash for periodic ML-DSA-87 signature.
- A `RoutingDecision` (allow / local-only / quarantine / deny) derived
  from the composer's `AggregateDecision`.
- A list of `RuleMatch` records, one per module that contributed.
- A `CorrelationFields` block matching §A.9 verbatim:
  `credential_event_id → lamprey_decision_id → mai_request_id`,
  plus tenant, subject_hash, service_identity, policy_version,
  trust_bundle_version, decision.

Verification: `AuditLog::verify_chain` detects link breaks, non-
monotonic IDs, nonzero head, and verifies periodic signatures using
the BF-3 `MlDsaBundleVerifier`. An off-host re-verifier needs only
the public verification key — no MAI source.

Triggers fire on:

- Violation thresholds (5 in 5 min → Warn, ×2 → Critical).
- Policy changes.
- Chain breaks (Critical).
- Storage-quota warnings (debounced on severity transitions).

The store keeps an in-memory `VecDeque` plus an optional JSON-lines
WAL, with a 7-year retention default (HIPAA), pluggable `StoreSealer`
(vault-AEAD wiring deferred), and a BF-5 offline correlation queue
(4096-event cap, drop counter) for SIEM degradation.

---

## Compliance reports

Source: `mai-compliance/src/reports/`, Session 43.

`ReportManager` ties the engine, certifier, template registry, and
pruner into one façade. Five built-in templates: HIPAA, ITAR, OCAP,
SystemActivity, MonthlyDigest. Custom templates register against
`ReportType::Custom`.

Every report carries a `TrustSection` (§A.13) regardless of template:
credential validation summary, trust bundle version history,
revocation snapshot mix, offline-interval reconstruction (from
correlation IDs), service-identity events, policy-version history,
audit verification status.

Certification: `ReportCertifier` wraps a `ReportDocument` into a
`CertifiedReport` with BLAKE3 content hash + optional ML-DSA-87
signature. The signed payload is canonical JSON, so the signature is
format-independent — an acquirer can re-render to HTML or CSV after
certification without invalidating the signature.

Retention: per-type defaults (HIPAA 7y, ITAR 7y, OCAP 10y,
SystemActivity 1y, MonthlyDigest 7y). Protected records never
auto-delete.

---

## Compliance dashboard

Source: `mai/compliance-dashboard/`, Session 44 + BF-6.

A FastAPI app with six pages:

| Page | Purpose |
|---|---|
| Overview | Module health, trust mode, policy version, audit chain head |
| Audit | Filter by tenant / module / decision / time; chain verification status |
| Reports | Generate / list / download HIPAA / ITAR / OCAP / Activity / Digest |
| Policy | View / update modules, apply templates, reload bundles |
| Alerts | Live SSE feed of policy decisions, violations, chain events |
| Health | Trust panel, scheduler health, system summary |

The admin gate uses `X-IM-Auth-Token: $MAI_DASHBOARD_ADMIN_TOKEN`
(default `dashboard-dev` for local-dev). The dashboard is the only
buyer-facing UI; everything else is API-driven.

---

## What sits above the inference layer

A subtle but important property: Lamprey does not modify, censor, or
transform inference content. It decides whether the inference is
allowed, where it can run, and what the audit record looks like. The
inference engine and the model are unchanged.

The compliance-routed reference scaffold (`apps/compliance-routed/`)
demonstrates this in one screen: a request arrives, the router
classifies, the composer decides, the audit log writes, the scheduler
places. If the composer says `deny`, the scheduler is never asked.

This separation is why Lamprey is portable: an acquirer can mount it
above their own inference stack without rewriting their model serving
layer. It is also why Lamprey is auditable: the decisions live in
their own crate, not buried inside a model-serving runtime.

---

## What an acquirer can verify in 30 minutes

1. Read `mai-compliance/src/lib.rs` and follow the module re-exports
   — the entire surface is visible at the top of one file.
2. `cargo test -p mai-compliance --lib` — 326+ green tests.
3. Walk `mai-compliance/src/policy/composer.rs` from `compose()`
   downward — the conflict-resolution logic is one function.
4. Open the dashboard, generate a HIPAA report, download it, verify
   the signature off-host using `verify_certified_report` from
   `mai-compliance::reports`.
5. Run `pytest apps/compliance-routed/tests/` — see the full
   router/composer/audit/scheduler loop in fixtures the operator can
   modify.

For deeper diligence, see the four demo scripts under
[`acquisition/demos/`](acquisition/demos/) — Healthcare, Defense,
Tribal, and Multi-domain.
