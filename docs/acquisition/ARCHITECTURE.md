# Acquisition Architecture Documentation

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Audience:** Acquirer technical diligence team, architecture review
boards, platform-team principals
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

This document is a top-down architectural reference for an acquirer
evaluating MAI + Lamprey for technology purchase, joint venture, or
strategic embed. It is intentionally narrower than
[`../MAI-MASTER-ARCHITECTURE.md`](../architecture/MAI-MASTER-ARCHITECTURE.md),
which is the engineering specification. This document focuses on
*what an acquirer needs to evaluate buy-vs-build*.

For positioning, see [`../ACQUISITION-PACKAGE.md`](../product/ACQUISITION-PACKAGE.md).
For competitive analysis, see [`COMPETITIVE.md`](COMPETITIVE.md).

---

## System diagram (text form)

```
+===========================================================+
|  L4-L5 Applications                                       |
|  (HomeBase, MedRecord, Tribal Health, Defence integrator, |
|   acquirer's own product)                                 |
+-----------------------------------------------------------+
|  MAI Python SDK + REST/SSE/WebSocket API                  |
|  (mai-sdk-python, mai-api)                                |
+===========================================================+
|                                                           |
|  LAMPREY COMPLIANCE GOVERNANCE STACK (the acquisition IP) |
|                                                           |
|  +-----------------------------------------------------+  |
|  | Layer 3 — Audit                                      | |
|  | hash-chained log (BLAKE3 + periodic ML-DSA-87)       | |
|  | reports (HIPAA/ITAR/OCAP/Activity/Digest)            | |
|  | dashboard (FastAPI, 6 pages)                         | |
|  +-----------------------------------------------------+  |
|  | Layer 2 — Policy                                     | |
|  | HIPAA + ITAR/EAR + OCAP modules                      | |
|  | composer (deny-wins, most-restrictive-route,         | |
|  |           OCAP > ITAR > HIPAA precedence)            | |
|  | decision cache (60s TTL, content-keyed)              | |
|  | templates (Standard/Healthcare/Defense/Tribal)       | |
|  +-----------------------------------------------------+  |
|  | Layer 1 — Router                                     | |
|  | classifier (PHI, controlled tech data, OCAP meta)    | |
|  | TrustContext-aware (BF-1/BF-2)                       | |
|  | binds to scheduler placement                         | |
|  +-----------------------------------------------------+  |
|                                                           |
+===========================================================+
|  MAI CORE — INFERENCE PLATFORM                            |
|                                                           |
|  +--------------+ +--------------+ +-----------------+    |
|  | scheduler    | | adapters     | | hardware iface  |    |
|  | (placement)  | | (Ollama,vLLM)| | (HIL: NVIDIA,   |    |
|  | (KV, batch,  | | (llama.cpp,  | |  AMD, CPU,      |    |
|  |  topology,   | |  TGI, TRT-LLM| |  TetraMem stub) |    |
|  |  power)      | |  ExLlama,    | |                 |    |
|  |              | |  SGLang)     | |                 |    |
|  +--------------+ +--------------+ +-----------------+    |
|                                                           |
+===========================================================+
|  TRUST MANIFOLD (BF-1..BF-6, three-ring)                  |
|                                                           |
|  ring 3: Local Trust Cache (every MAI node)               |
|  ring 2: Lamprey Trust Bridge (per tenant)                |
|  ring 1: OpenBao Trust Core (single org)                  |
|                                                           |
+===========================================================+
|  IM-OS BASE (vault, ZFS, TPM, PQC, network policy)        |
|  (mai-vault, OS layer)                                    |
+===========================================================+
```

The dashed equals lines are deployment boundaries — every layer above
the line can be replaced by an acquirer without recompiling the
layers below. The Lamprey block is the IP an acquirer buys.

---

## Integration points

### Where Lamprey plugs into existing inference infrastructure

The Lamprey router is the supported integration seam. It accepts:

| Input | Type | Source |
|---|---|---|
| `RequestMetadata` | tenant, subject, model, content classification hints | acquirer's intake layer |
| `TrustContext` | BF-1 shape (claim_id, tenant_id, subject_hash, scopes, allowed routes) | acquirer's OpenBao Trust Bridge |
| `ConnectivityState` | Connected / Degraded / AirGapped | acquirer's network policy reader |
| `PolicyBundleVersion` | semver | acquirer's policy distribution |
| `ClassificationResult` | PHI flags, controlled-tech indicators, OCAP metadata | acquirer's classifier or Lamprey's default |

And returns:

| Output | Type | Consumed by |
|---|---|---|
| `AggregateDecision` | allow / local_only / quarantine / deny + reasons + flags | scheduler placement |
| `AuditEntry` | hash-chained + correlation IDs | acquirer's SIEM (metadata) + local store |
| `RoutingHint` | preferred backend tier (cloud / local / sentinel) | scheduler / router |

The acquirer wires the inputs and consumes the outputs. The Lamprey
crate ships with all four module engines, the composer, the audit
chain, the report generator, and the dashboard. The acquirer wires
their own classifier in if they prefer.

### Where MAI plugs into existing inference infrastructure

The MAI core is more invasive — it expects to own placement and
adapter lifecycle. The integration paths are:

1. **Replace the placement layer.** Use `mai-scheduler` as the
   request placement engine and adapt the acquirer's existing
   adapters to the `Adapter` trait.
2. **Wrap as a backend tier.** Treat MAI as a single "secure local
   inference" tier behind the acquirer's existing model gateway.
   Lamprey routing decides when a request enters this tier.
3. **Acquire selectively.** An acquirer interested only in Lamprey
   compliance routing can take just the `mai-compliance` and
   `mai-sdk-python` crates and run them above their existing
   inference stack.

Option 3 is the cleanest acquisition path: Lamprey is independent of
the MAI scheduler in source code (no shared types beyond the
`AggregateDecision` shape) and can be lifted out.

---

## Data flow per compliance scenario

### Healthcare (HIPAA)

```
1. User auth     → Trust Bridge mints claim (tenant=clinic, scope=hipaa)
2. Local API     → SDK forwards claim + prompt to mai-api
3. Lamprey       → router classifies; PHI detected; HIPAA module fires
4. Composer     → local_only_allowed (PHI requires local route)
5. Scheduler     → places on local instance
6. Inference     → adapter runs the local model
7. Response      → returned to user
8. Audit         → entry written with correlation IDs; report aggregator updates
```

No payload crosses the cloud trust boundary. The HIPAA report
generator produces a signed PDF/JSON output the operator can hand to
a compliance officer.

### Defence (ITAR/EAR)

```
1. User auth      → Trust Bridge mints claim (scope=itar_ear, jurisdiction=US)
2. Local API      → SDK forwards claim + prompt
3. Lamprey        → router classifies; controlled tech data detected
4. ITAR module    → backend eligibility: only US air-gapped instances qualify
5. If no qualifier → deny; if one qualifies → local_only_allowed
6. Scheduler      → places on qualifying instance (if any)
7. Audit          → entry includes jurisdiction + scope; quarantine if unclear
```

The composer refuses if no instance satisfies the ITAR backend
eligibility filter.

### Tribal sovereignty (OCAP)

```
1. User auth      → Trust Bridge mints claim (scope=ocap, role=elder)
2. Local API      → SDK forwards claim + prompt + OCAP metadata
3. OCAP pipeline  → 9 stages (scope, revocation, possession, ...)
4. Composer       → local_only_allowed (or refusal at any stage)
5. Scheduler      → places on tribal-controlled instance
6. Audit          → records every stage decision for tribal records
```

The `apps/tribal-sovereignty/` scaffold demonstrates the typical
sequence with sovereignty-violation errors when an authority
mismatch occurs.

### Multi-domain conflict (HIPAA + OCAP)

```
1. Request triggers both modules.
2. HIPAA returns local_only (PHI present).
3. OCAP returns local_only (tribal data + consent).
4. Composer folds: most-restrictive-route = local_only.
5. Deny-wins: not triggered (neither denied).
6. Precedence chain: OCAP > HIPAA; in case of reason conflict,
   OCAP reason takes the headline; HIPAA reason carried as
   secondary in the audit entry.
7. Audit log: both reason codes recorded, both module versions
   stamped.
```

The audit row carries an explanation array with both contributors,
so a regulator can see exactly which modules acted.

---

## Key architectural decisions and rationale

### 1. Scheduler placement is the inference contract

The scheduler is the entrypoint for every inference request. Adapters
do not accept direct calls; the scheduler hands a request to an
adapter. Rationale: hardware-aware placement is the property that
survives a hardware generation refresh, and the scheduler is the
only place to centralise that knowledge.

### 2. Lamprey sits above inference, not inside it

The composer returns an `AggregateDecision` *before* the scheduler is
consulted. A denied request never reaches the inference layer.
Rationale: makes the compliance behaviour auditable from a single
crate, makes it portable to acquirer-owned inference stacks, makes
the policy logic testable without an inference dependency.

### 3. Trust Manifold separates identity from payload

The OpenBao-backed trust layer carries identity, claims, signatures,
revocation snapshots, and audit correlation IDs only. Prompts and
completions never traverse it. Rationale: regulated organisations
cannot accept a credential system that also reads their payloads.

### 4. Air-gap is a routing input, not a deployment flag

The router and the composer read `ConnectivityState` on every
request. Rationale: air-gap intermittency is a real operational mode
(rural hospital, treaty-land deployment, defence drill), not a
configuration that flips once at startup.

### 5. Audit log is hash-chained with periodic PQC signatures

BLAKE3 chain + ML-DSA-87 signatures over canonical JSON. Rationale:
post-quantum cryptography requirement for defence and federal
deployments, off-host re-verification property for regulator
inspection.

### 6. OCAP is a first-class compliance module, not a flag

The 9-stage OCAP pipeline encodes tribal data sovereignty as a
governance contract rather than as a keyword filter. Rationale: no
other AI infrastructure vendor supports OCAP; tribal procurement is
an under-served market with binding sovereignty requirements.

### 7. Reports are format-independent signed JSON

Certification signs the canonical JSON rendering; HTML / CSV / Text
are derived after signing. Rationale: regulators ask for different
formats; re-signing every format would be a key-rotation nightmare.

---

## Trust boundaries

The diagram is intentionally simple:

```
+-------------------+
|  Acquirer's IdP   |    (Okta / Azure AD / Auth0 / workload identity)
+---------+---------+
          |
          v
+-------------------+
|  Lamprey Trust    |    (per-tenant; runs on acquirer's infra)
|  Bridge           |    mints claims, signs with OpenBao Transit key
+---------+---------+
          |
          v   (claims + signatures + revocation snapshots only)
+--------------------------------+
|  Local Trust Cache (BF-4)      |    (every MAI node)
|  - verifies signatures         |
|  - tracks freshness            |
|  - exposes /v1/trust/status    |
+----------+---------------------+
           |
           v
+--------------------------------+
|  Lamprey policy + audit + reports + dashboard |
|  (every decision carries TrustContext)        |
+----------+------------------------------------+
           |
           v
+--------------------------------+
|  MAI scheduler placement       |
+--------------------------------+
```

The cleanest acquisition claim is that **the cloud-side trust core
never sees payloads**. The bridge mints claims; the cache verifies
them; everything regulated stays inside the local boundary.

---

## Source-of-truth navigation

| Layer | Crate / package | Entry points |
|---|---|---|
| Applications | `apps/` | Six reference scaffolds (see plan §"Sessions 30-31") |
| SDK | `mai-sdk-python` | `client.py`, `async_client.py`, `_namespaces.py` |
| API | `mai-api` | `routes.rs`, `handlers/*` |
| Audit (Lamprey L3) | `mai-compliance/src/audit/` | `mod.rs`, `chain.rs`, `store.rs` |
| Reports | `mai-compliance/src/reports/` | `mod.rs`, `engine.rs`, `pdf.rs` |
| Policy (Lamprey L2) | `mai-compliance/src/policy/` | `composer.rs`, `cache.rs`, `templates.rs` |
| HIPAA module | `mai-compliance/src/hipaa/` + `phi.rs`, `deid.rs`, `baa.rs`, `medical_entities.rs` | |
| ITAR/EAR module | `mai-compliance/src/{itar,ear,jurisdiction,tech_data}.rs` | |
| OCAP module | `mai-compliance/src/ocap/` | `mod.rs`, `tribal_data.rs`, `treaty.rs`, `cultural.rs`, `ocap_rules.rs` |
| Trust Manifold | `mai-compliance/src/{trust,trust_cache,bundle,subject_hash}.rs` | |
| Router (Lamprey L1) | (binds via `mai-compliance` to scheduler) | |
| Scheduler | `mai-scheduler` | `default.rs`, `scoring/`, `topology/`, `kv/`, `batch/` |
| Adapters | `mai-adapters` + `adapters/*` | seven backends |
| HIL | `mai-hil` | NVIDIA / AMD / CPU / TetraMem (stub) drivers |
| Vault | `mai-vault` | ZFS / PQC / TPM / audit chain |
| Dashboard | `mai/compliance-dashboard/` | FastAPI 6-page app |
| Deployment | `mai/deployment/{local-dev,cloud-trust-core,local-mai-node,airgap-demo}/` | profile.toml + README |

Every layer in the table is either a separate crate or a clear
sub-tree. An acquirer doing a buy-vs-build evaluation should walk
this table top to bottom; the buy-vs-build decision is different at
each layer.

---

## What this document is not

- It is not a hardware guide — see `configs/{scout,ranger,pack-leader}.toml`.
- It is not a deployment runbook — see [`../DEPLOYMENT.md`](../operations/DEPLOYMENT.md).
- It is not a security audit — see [`../SECURITY.md`](../compliance/SECURITY.md).
- It is not the original spec — see [`../MAI-MASTER-ARCHITECTURE.md`](../architecture/MAI-MASTER-ARCHITECTURE.md).

This document is the architecture overlay an acquirer reads first to
decide which parts of MAI/Lamprey they want to acquire, and then
fans out into the layer-specific briefs:

- [`../SCHEDULER-BRIEF.md`](../architecture/SCHEDULER-BRIEF.md)
- [`../LAMPREY-BRIEF.md`](../product/LAMPREY-BRIEF.md)
- [`../AIR-GAP-BRIEF.md`](../product/AIR-GAP-BRIEF.md)
- [`../API-REFERENCE.md`](../api/API-REFERENCE.md)
- [`../SDK-REFERENCE.md`](../api/SDK-REFERENCE.md)
- [`../TRUST-MANIFOLD.md`](../compliance/TRUST-MANIFOLD.md)
- [`../LOCAL-TRUST-CACHE.md`](../compliance/LOCAL-TRUST-CACHE.md)
- [`../AUDIT-CORRELATION.md`](../compliance/AUDIT-CORRELATION.md)
- [`COMPETITIVE.md`](COMPETITIVE.md)
- [`IP.md`](IP.md)
- [`INTEGRATION.md`](INTEGRATION.md)
- [`demos/`](demos/) — four reproducible compliance demos
