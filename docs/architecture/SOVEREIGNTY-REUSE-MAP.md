# Sovereignty Stack — MAI Reuse Map (Prompt 0.1)

**What this is:** the authoritative map of which existing Lamprey MAI source feeds each new
WSF/AOG crate and service. Governs Phases F/W/G/T. Extraction is **in-place**: new crates live in
this workspace; `mai-compliance` and friends re-export from them so their existing tests keep
passing. No fork, no second repo.

**Legend**
- `EXTRACT` — move code into a new standalone crate; original crate re-exports (zero behaviour change).
- `PROMOTE` — lift an internal module into a shared crate consumed by ≥2 products.
- `REUSE` — depend on the existing crate as-is.
- `EXTEND` — new type/behaviour layered on an existing schema (superset).
- `NEW` — no MAI antecedent; built from scratch.
- `PATTERN` — reuse the design, not the code (e.g. React console vs Jinja2 dashboard).

---

## Fabric crates (Phase F)

| New crate | MAI source | Kind | Coupling to sever / notes |
|---|---|---|---|
| `fabric-contracts` | claim schema (`docs/compliance/TRUST-MANIFOLD.md` §4), `AuditEntry` (`mai-compliance/src/audit/entry.rs`) | EXTEND | Pure types only; no logic. The four schema docs in `contracts/` are its spec. |
| `fabric-proof` | `mai-compliance/src/audit/{chain,entry,sealer,store,mod}.rs`, `mai-compliance/src/bundle.rs`, `mai-compliance/src/subject_hash.rs` | EXTRACT | Dep-light target: blake3, ml-dsa, hmac, sha2, serde, chrono, thiserror. `mai-compliance` re-exports; its 326+ tests are the regression guard. **Blocked on 0.2** (ml-dsa timing fix). |
| `fabric-identity` | TLM PKI issuance (`mai-api` TLM-2 commits), `docs/compliance/SERVICE-IDENTITY.md`, `mai-compliance::subject_hash` | EXTRACT+NEW | Reuse PKI leaf issuance + the nine service identities; new session/task identity nesting. |
| `fabric-token` | `SignedClaim` (fabric-contracts) | EXTEND | Claim + budget strand + attenuation. Wire-superset of the MAI claim (see `contracts/trust-token.md`). |
| `fabric-envelope` | `mai-vault` `AeadSealer` (AES-256-GCM), mai-compliance classifiers (`phi`,`itar`,`ocap`,`tech_data`) | REUSE+NEW | Seal wrap reuses AeadSealer + OpenBao transit; label wrap reuses classifiers; thread wrap is new. |
| `fabric-cache` | `mai-compliance/src/trust_cache` (connectivity state machine) | PROMOTE | Shared offline-verification crate. |
| `fabric-revocation` | TLM revocation refresh loop + snapshot signing | PROMOTE | Signed snapshots + `emergency_revoke`; offline apply from removable media. |

## WSF services (Phase W)

| Service | MAI source | Kind | Notes |
|---|---|---|---|
| `wsf-bridge` (Ring 2) | TLM Trust Bridge (TLM-1..4.2: AppRole, PKI, Transit sign, rotation), `handlers/trust.rs::exchange_token`, `TrustExchangeMode` | PROMOTE | The credential-rotation E2E (TLM-4.2) is the proof this ring works. Productize to HA/stateless. |
| `wsf-broker` (cloud STS) | — | NEW | AWS STS AssumeRole+session policy / GCP generateAccessToken / Azure workload identity. Root creds in OpenBao `kv`. TLM rotated MAI's *own* creds; brokering *customer-cloud* creds is net-new. |
| `wsf-seal` | `fabric-envelope` + OpenBao transit | REUSE | Network wrapper for seal/unseal. |
| `wsf-ledger` | `fabric-proof` + `mai-compliance/src/reports/*` (certified report generator) | REUSE | Receipt ingest/verify + signed evidence-pack export. |
| Ring 3 cache daemon | `fabric-cache` + `fabric-revocation` | REUSE | Appliance-side; connectivity state drives route ceiling. |

## AOG services (Phases G/T)

| Service | MAI source | Kind | Notes |
|---|---|---|---|
| `aog-gateway` routing | `mai-router/src/{classifier,entities,cost,fallback,pipeline,router,rules}.rs` | REUSE | Local-vs-cloud decision; `DefaultRouter`, `RouteRequest`, `RoutingDecision`. |
| `aog-gateway` policy | `mai-compliance/src/policy/composer.rs` + engines (`phi`,`baa`,`deid`,`itar`,`ear`,`jurisdiction`,`ocap`) | REUSE | Deny-wins composer is the policy engine (no Cedar/OPA in v1). Add a `destination` policy module. |
| `aog-gateway` providers | — (MAI's "cloud" was a `router.toml` label; no client code) | NEW | vLLM/Ollama via existing MAI adapters; Anthropic + OpenAI clients are new. |
| `aog-gateway` egress redaction | `mai-compliance::{deid,phi,itar}` | REUSE | Tokenize/redact on cloud egress + tool-result egress. |
| `aog-toolproxy` | `mai-agent::{ToolRegistry,ToolAccessRole,ToolAuditEntry,ResourceBudget,TokenAccounting}` | IMPLEMENT SEAM | MAI defined these as interface-only ("L4 responsibility"). AOG is that L4. |
| `aog-approvals` | — (pattern: Lamprey Harness permissions-store + approval chips) | NEW/PATTERN | Human approval inbox; shared by AOG tools, WSF cred grants, Aeneas remediations. |
| `aog-meter` | receipts carry `spend_cents`,`workflow_id` (fabric-contracts) | NEW | Aggregation + ROI/break-even + recommender. |

## Console (Phase C)

| Surface | MAI source | Kind | Notes |
|---|---|---|---|
| `console/` (React 19 + TS + Vite + Tailwind) | `compliance-dashboard/` (FastAPI+Jinja2) — **feature reference only** | PATTERN | Ports the useful views (overview, audit search, policy toggles, monitoring); UX DNA from Lamprey Harness panels. Jinja2 dashboard retired when C reaches parity. |

---

## Extraction ordering constraints
1. `0.2` (ml-dsa timing + axum Handler) **before** `fabric-proof` (F1).
2. `fabric-contracts` (0.8) **before** any crate that types tokens/receipts/envelopes.
3. `fabric-proof` (F1) **before** `fabric-token`/`fabric-envelope`/`wsf-ledger` (they sign/chain via it).
4. `wsf-bridge` + `wsf-broker` (W1–W2) **before** `aog-gateway` virtual-key→token (G1) and `aog-toolproxy` per-call creds (T2).

## Parked — do NOT touch in this initiative (revive with Summit)
- `mai-scheduler` (and its known fake-metrics defects: `default.rs:{394,399,402}`, `manager.rs:592`, `usb.rs:161`).
- `mai-hil`, and inference adapters beyond what `aog-gateway` G2 needs.
- L5 family-app scaffolds (`apps/summit-chat`, `familyvault`, `scribe`, `legacy-engine`, `medrecord`, `homebase`, `estate-ai`).

## Confirmed-open defects that this initiative MUST fix (not inherit)
- RUSTSEC-2025-0144 (ml-dsa timing) — trust-critical, blocks F1 (Prompt 0.2).
- axum 0.7/0.8 dual-`Handler` — resurfaces across the gateway surface (Prompt 0.2).
- gRPC `scan_models` / gRPC+WS streaming placeholders — only relevant if those surfaces are reused; AOG uses its own axum surface, so these are parked with the scheduler unless a consumer appears.
