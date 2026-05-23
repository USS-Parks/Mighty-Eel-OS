# IP Position Memo

**Project:** Island Mountain Model Abstraction Interface (MAI) +
Lamprey
**Audience:** Acquirer corporate-development, IP counsel, technology
diligence
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

> **Disclaimer.** This document is engineering's view of the
> patentable, trade-secret, and licence-boundary content in the MAI
> + Lamprey codebase. It is not legal advice. An acquirer should
> have qualified patent counsel evaluate every claim below against
> the prior-art record before relying on it for valuation.

---

## Summary of position

The acquisition IP is concentrated in **Lamprey**
(`mai-compliance/`) plus the **Trust Manifold + Local Trust Cache**
(`mai-compliance/{trust,trust_cache,bundle}.rs`). MAI itself
(scheduler, HIL, adapters) is engineering of high quality but
operates in well-trodden territory; the moat in MAI is the
discipline of the implementation, not novel invention.

The candidate inventions below are the four most patent-defensible
constructs in the codebase. They are also the four most
operationally distinctive features versus the competitive landscape
in [`COMPETITIVE.md`](COMPETITIVE.md).

---

## Patent candidate 1 — Multi-domain compliance routing for AI inference

**Construct:** A routing system that consults independent
regulatory-domain policy modules (HIPAA, ITAR/EAR, OCAP), normalises
their outputs to a shared decision shape, and resolves conflicts via
deny-wins, most-restrictive-route, and a configurable precedence
chain — producing a single placement decision that the inference
scheduler then executes.

**Where it lives:** `mai-compliance/src/policy/composer.rs` (Session
41). `ModuleDecision`, `AggregateDecision`,
`AggregateDecision::compose`, precedence-chain logic.

**Why it is novel:**

- Existing AI gateway products run safety filters sequentially with
  ordering as the only conflict mechanism (e.g., NeMo Guardrails,
  Guardrails AI).
- Existing policy engines (OPA / Minder) compose policies but do not
  specialise on regulatory-domain semantics — a HIPAA "local-only"
  decision and an ITAR "deny" have different operational
  consequences and require different reason taxonomies.
- The composer's most-restrictive-route fold is a non-obvious choice
  — it requires defining a partial order on routing classes (cloud
  > local-only > deny) and applying it across modules that do not
  share a common vocabulary natively.

**Prior art to consider:**
- Generic OPA / Rego policy composition.
- XACML combining algorithms (deny-overrides, permit-overrides).
- AWS IAM policy evaluation.

**Engineering view:** The novelty is in the *combination* of
regulatory-domain-specific modules + a normalised decision shape +
a route-restrictiveness lattice. None of the cited prior art
addresses all three for AI inference routing.

---

## Patent candidate 2 — OCAP-native AI inference governance engine

**Construct:** A nine-stage decision pipeline that evaluates a
request against tribal data sovereignty principles (Ownership,
Control, Access, Possession) plus treaty consent and cultural
consent — encoded as typed errors at each stage and stamped onto
the audit record with sovereignty correlation fields.

**Where it lives:** `mai-compliance/src/ocap/` — `mod.rs`,
`tribal_data.rs`, `treaty.rs`, `cultural.rs`, `ocap_rules.rs`
(Session 40). `OcapEvaluator::evaluate(...)` runs the pipeline.

**Why it is novel:**

- No competing AI infrastructure vendor has shipped an OCAP module.
  Search across patent databases for "OCAP" + "inference" or
  "OCAP" + "machine learning" returns zero hits (as of last sweep).
- The combination of tribal-source possession evaluation +
  authorised-profile control checks + sacred / elder role gates +
  cultural and treaty consent in a single pipeline is unique.
- The fail-closed semantics (missing scope refuses with
  `OcapError::ScopeMissing`) is a non-trivial design choice;
  defaulting to allow would be the obvious implementation.
- Integration with the broader composer (OCAP > ITAR > HIPAA
  precedence chain) ties the OCAP decisions into a multi-domain
  audit record.

**Prior art to consider:**
- Generic consent-management systems (One Trust, etc.).
- Data classification systems (e.g., AWS Macie).
- FNIGC OCAP doctrine itself (a governance framework, not a
  software construct).

**Engineering view:** The patentable construct is the *encoding of
OCAP into a deterministic decision pipeline integrated with
inference-time placement*. The OCAP doctrine is decades old;
embedding it in inference governance is novel.

---

## Patent candidate 3 — Hash-chained AI compliance audit with periodic PQC signatures

**Construct:** An append-only audit log where each entry's hash
chains to the previous via BLAKE3, with periodic ML-DSA-87
signatures over the canonical-JSON rendering of a batch. The
signature is format-independent so the audit can be re-rendered to
HTML, CSV, or PDF without invalidating the proof. Off-host
verification requires only the public verification key.

**Where it lives:** `mai-compliance/src/audit/{entry,chain,store}.rs`
(Session 42), `mai-compliance/src/reports/pdf.rs` (Session 43 —
certification helper).

**Why it is novel:**

- The combination of BLAKE3 link chaining + ML-DSA-87 periodic
  signatures + format-independent canonical-JSON signing is not
  matched by existing audit-log products.
- Off-host re-verification without trusting the originating product
  is a regulator-friendly property absent from existing AI audit
  logs (which are typically application logs, sometimes append-only
  via Kinesis or similar, but never signed with PQC).
- ML-DSA-87 selection reflects 2024 NIST PQC standardisation;
  using a post-quantum signature for audit logs that need to remain
  verifiable in 2050+ is forward-looking.

**Prior art to consider:**
- Certificate Transparency logs (Merkle trees with periodic
  signatures).
- Linux journald with FSS (Forward Secure Sealing).
- AWS QLDB (ledger database).
- Bitcoin / blockchain-style chains.

**Engineering view:** The construct is not a Merkle tree (it is a
linear chain), it is not a generic ledger (it is purpose-built for
compliance decisions), and the PQC choice + canonical-JSON signing
distinguish it from existing event-log products. Patentability would
turn on the precise claim language.

---

## Patent candidate 4 — Hardware-aware compliance routing with physical air-gap enforcement

**Construct:** An inference routing system in which physical
air-gap state (hardware switch or operator flag) is a first-class
input to the policy composer, such that air-gap engagement
deterministically restricts route choices (cloud → local-only, or
deny) and the decision is recorded with both the air-gap state and
the originating credential event in a tamper-evident audit log.

**Where it lives:** `mai-core/src/airgap/` (Session 28) +
`mai-compliance/src/policy/composer.rs` (composer integration) +
`mai-compliance/src/trust_cache.rs` (BF-4, five-state model) +
`mai-api/src/server.rs` (loopback-bind enforcement) +
`mai-compliance/src/audit/` (audit linkage).

**Why it is novel:**

- AI gateway products do not typically support air-gap as a deployment
  mode at all; air-gap deployment + air-gap as a *routing input* is
  not a standard category.
- The combination of hardware-switch reading + software flag + audit
  correlation is unusual.
- Loopback-bind enforcement *as a config-validation guard before
  listener open* is a defensive construct rarely seen in standard
  HTTP server configurations.

**Prior art to consider:**
- Hardware Security Modules (HSMs) with policy enforcement.
- Air-gap deployment guides for various enterprise software.
- "Restricted-mode" features in some compliance products.

**Engineering view:** The novelty is in tying air-gap state to
inference-time routing in a way that is both deterministic and
auditable. Air-gap as a deployment posture is well-known; air-gap
as a continuously-consulted routing input is less common.

---

## Trade secrets

These are NOT patent candidates but ARE proprietary value drivers:

### Pattern dictionaries
- `mai-compliance/src/medical_entities.rs` — the medical entity
  recognition dictionary (HIPAA PHI surface).
- `mai-compliance/src/phi.rs` — the PHI pattern set across the 18
  HIPAA identifier categories.
- `mai-compliance/src/tech_data.rs` — controlled technical data
  indicator set (TAA / USML hints).
- `mai-compliance/src/ocap/cultural.rs` — cultural-data
  classification cues.

These are accumulated knowledge — they will keep improving in
versioned releases and the buyer benefits from the maintenance
pipeline.

### Rule sets
- The policy template defaults
  (`mai-compliance/src/policy/templates.rs`): Standard, Healthcare,
  Defense, TribalGovernment.
- The OCAP nine-stage rule ordering and per-stage reason codes.
- The composer's precedence chain (OCAP > ITAR > HIPAA) and the
  most-restrictive-route fold logic.

These were authored from regulatory interpretation. Reverse-engineering
them is possible but not free.

### Entity detection models
- The compiled regex sets and lookup tables backing PHI / OCAP /
  controlled-tech detection.

### Architecture documentation
- The plan
  (`BUILD-EXECUTION-PLAN-V2-UPDATED.md`), the roster
  (`MAI-BUILD-PROMPT-ROSTER-v2.md`), the per-phase decision logs in
  `docs/SESSION-LOG.md`, the
  `docs/{TRUST-MANIFOLD,OPENBAO-INTEGRATION,SERVICE-IDENTITY,TRUST-BUNDLE-SPEC,LOCAL-TRUST-CACHE,AUDIT-CORRELATION}.md`
  series.

This documentation is the institutional memory; rebuilding it from
the code alone would take months and would not capture the
trade-off rationale.

---

## Open-source vs proprietary boundary

The repository as a whole has not been published. An acquirer
decision on what to open and what to keep proprietary is a
post-acquisition concern, but the natural seam is:

| Layer | Suggested posture |
|---|---|
| HIL, adapters, scheduler (MAI core) | Open-source friendly — these are infrastructure plumbing. Releasing them widens the ecosystem and feeds back hardware support contributions. |
| Trust Manifold spec docs | Open spec, proprietary implementation. The spec is more valuable as a standard others adopt; the implementation is the differentiation. |
| Lamprey composer + audit chain | Proprietary. This is the core IP. |
| HIPAA / ITAR / OCAP modules | Proprietary. The pattern dictionaries and rule sets are the trade secrets. |
| Policy templates | Proprietary defaults; customers may supply their own templates. |
| Reports / dashboard | Proprietary. |
| Reference scaffolds (`apps/`) | Open. They demonstrate integration without exposing module internals. |
| SDK (`mai-sdk-python`) | Open. SDK adoption is friction-reduction; closed SDKs hurt sales. |

This split mirrors how successful infrastructure businesses balance
ecosystem growth with monetisable IP (HashiCorp's open-core posture,
Snowflake's proprietary database + open clients, Confluent's split
across Kafka and proprietary plug-ins).

---

## What an acquirer's IP counsel should focus on

In priority order:

1. **OCAP module patentability** — likeliest novel claim because the
   prior art space is sparse. Verify zero patent literature on
   "tribal data sovereignty inference governance" or close
   synonyms.
2. **Composer precedence + most-restrictive-route fold** —
   patentability turns on claim language; consider both a method
   patent on the composition algorithm and a system patent on the
   composer-+-modules-+-audit chain combination.
3. **Air-gap routing input** — assess novelty against HSM policy
   literature and DoD air-gap guidance.
4. **PQC-signed audit chain** — likeliest to face prior-art
   challenges from Certificate Transparency and Linux journald FSS;
   patentability turns on the format-independent canonical-JSON
   signing approach.
5. **Trade-secret protection regime** — ensure the pattern
   dictionaries, rule sets, and template defaults are protected
   under standard trade-secret practices (access controls, NDA
   coverage, build-pipeline access logging).
6. **Open-source licence audit** — confirm no copyleft dependency in
   the proprietary crates (`mai-compliance`, in particular). The
   repository is Cargo-managed; `cargo-license` should produce a
   clean MIT/Apache-2.0/BSD picture.

---

## What this memo does not cover

- Trademark position on "Lamprey", "Island Mountain", "MAI"
  (separate trademark counsel review required).
- International patent strategy (US vs PCT vs targeted nationals).
- Defensive patent strategy or membership in patent pools.
- Licensing of inbound third-party crates (handled in standard
  open-source-due-diligence flow; `cargo deny` configuration is in
  the repository root).

These belong to a full IP audit, not this engineering memo.
