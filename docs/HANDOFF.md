# MAI Founding Engineer Handoff

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Source:** MAI-BUILD-PROMPT-ROSTER-v2.md (46 sessions with Lamprey compliance governance) + `BUILD-EXECUTION-PLAN-V2-UPDATED.md` Appendix A (Trust Manifold backfill BF-1..BF-7).
**Status:** Sessions 1-30 and 32-46 complete; **Gate C closed**; **Trust Manifold backfill lane closed** (2026-05-22); **Gate D closed** (Session 46); **Ship hardening lane SHIP-01..SHIP-17 closed** (2026-05-23); **RC1 lane RC-01..RC-10 closed** with an outside-tester bundle shipped 2026-05-24. Active work is the DOUGHERTY remediation lane (J-01..J-26) before RC-11 re-bundle/re-ship; see `docs/INDEX.md`, `docs/COGENT-DEPLOYMENT-ROADMAP.md`, and `docs/dougherty/JOHN-REMEDIATION-PLAN.md`.
**Archive:** Sessions 01-10 archived to `SESSION-LOG-ARCHIVE-01.md`; Sessions 11-25 archived to [SESSION-LOG-ARCHIVE-02.md](sessions/SESSION-LOG-ARCHIVE-02.md) on 2026-05-23.

---

## What You Are Picking Up

MAI is the core inference abstraction layer for IM-OS, Island Mountain's sovereign data and identity operating system. It sits between all application logic (chat, document AI, medical records, home automation, legacy preservation) and all inference hardware (NVIDIA GPUs today, AMD GPUs, classical memristors in 2028, quantum memristors in 2030+).

The inference engine is a plugin. The data sovereignty layer is the product.

This handoff is intentionally half orientation and half guardrail. It tells an incoming engineer what exists, where to start, what not to break, and which documents carry the authoritative session-level history.

---

## Where To Start

Four roles, four paths. Find yours and follow it before reading anything else.

**Incoming engineer (60 min):** Read this document top to bottom. Then read [ARCHITECTURE.md](../ARCHITECTURE.md) for the trust model and component catalog, [CONVENTIONS.md](../CONVENTIONS.md) for quality gates and monorepo layout, and [SESSION-RULES.md](../SESSION-RULES.md) for session governance. Open [SESSION-LOG.md](sessions/SESSION-LOG.md) to see the active work items. The "Five Things That Will Bite You" section below is the most important part of this document -- read it twice.

**Acquirer technical reviewer (30 min):** Read "What You Are Picking Up" and the component table in "Current State." Then go directly to [ACQUISITION-PACKAGE.md](product/ACQUISITION-PACKAGE.md) for the five defensible points with code citations, [BUYER-INTEGRATION-GUIDE.md](product/BUYER-INTEGRATION-GUIDE.md) for the security architect review path, and [DEMO-SUITE.md](product/DEMO-SUITE.md) to run the Trust Manifold scenario. You do not need the detailed session history unless you are auditing provenance.

**Security architect (45 min):** Start with the trust boundary section of [BUYER-INTEGRATION-GUIDE.md](product/BUYER-INTEGRATION-GUIDE.md). Work through its review checklist. Then run `pytest apps/openbao-trust-demo/tests/` to confirm the live trust loop. The policy composer (`mai-compliance/src/policy/composer.rs`) and audit store (`mai-compliance/src/audit/store.rs`) are the two modules most worth reading in full.

**Executive or sales reviewer (15 min):** Read [ACQUISITION-PACKAGE.md](product/ACQUISITION-PACKAGE.md) only. If the five defensible points land, ask for the combined acquisition demo in [DEMO-SUITE.md](product/DEMO-SUITE.md).

---

## Current State

### Governance Documents

| Document | Purpose |
|---|---|
| [MAI-BUILD-PROMPT-ROSTER-v2.md](../MAI-BUILD-PROMPT-ROSTER-v2.md) | All 46 session prompts, deliverables, and acceptance criteria. |
| `BUILD-EXECUTION-PLAN-V2-UPDATED.md` (repo root) | Governing execution plan including Appendix A Trust Manifold backfill. |
| [ARCHITECTURE.md](../ARCHITECTURE.md) | Trust model, component catalog, data flows. |
| [CONVENTIONS.md](../CONVENTIONS.md) | Code quality gates, monorepo layout, testing rules. |
| [SESSION-LOG.md](sessions/SESSION-LOG.md) | Active progress tracker. |
| [SESSION-LOG-ARCHIVE-01.md](../SESSION-LOG-ARCHIVE-01.md) | Completed sessions 01-10. |
| [SESSION-LOG-ARCHIVE-02.md](sessions/SESSION-LOG-ARCHIVE-02.md) | Completed sessions 11-25, archived 2026-05-23. |
| [SESSION-RULES.md](../SESSION-RULES.md) | Dependency enforcement, acceptance criteria, quality gates. |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Deferred work, limitations, open questions. |
| [INDEX.md](INDEX.md) | Master file index. |
| [ACQUISITION-PACKAGE.md](product/ACQUISITION-PACKAGE.md) | Five-point buyer thesis with code/test citations. |
| [BUYER-INTEGRATION-GUIDE.md](product/BUYER-INTEGRATION-GUIDE.md) | OpenBao-backed trust boundary and integration sequence. |
| [DEMO-SUITE.md](product/DEMO-SUITE.md) | Trust Manifold scenario and supporting demos. |
| [SHIP-HARDENING-PLAN.md](sessions/SHIP-HARDENING-PLAN.md) | Closed SHIP-01..SHIP-17 hardening sequence; the lane on top of S1..S46 that removed demo-safe defaults. |
| [SHIP-PROFILE.md](operations/SHIP-PROFILE.md) | Production-profile contract; status table tracks per-SHIP enforcement. |
| [COGENT-DEPLOYMENT-ROADMAP.md](product/COGENT-DEPLOYMENT-ROADMAP.md) | RC1 → RC2 → appliance release ladder bridging Gate D to a shippable installer. |

### Codebase Baseline

The workspace contains the full MAI crate family: `mai-core`, `mai-vault`, `mai-agent`, `mai-scheduler`, `mai-compliance`, `mai-router`, `mai-hil`, the Python SDK, and 7 backend adapter implementations. The REST/gRPC API, vault interfaces, agent/RAG layer, scheduler foundation, topology graph, KV cache manager, continuous batching engine, scoring module, metrics feedback loop, simulator, power/sentinel path, model install/remove seams, OTA/model lifecycle layer, compliance engines, policy runtime, audit chain, reports, dashboard, deployment profiles, and demo scaffolds are present.

| Subsystem | Primary location | Status | Evidence |
|---|---|---|---|
| Inference core, API, adapters, streaming, auth | `mai-api/`, `mai-adapters/`, `mai-sdk-python/` | Complete | REST/gRPC/SSE/WebSocket + SDK tests. |
| Hardware-aware scheduler | `mai-scheduler/src/` | Complete | Topology, KV, batching, scoring, metrics, simulator. |
| Vault and PQC crypto | `mai-vault/`, `mai-core/src/vault.rs` | Complete | ML-KEM/ML-DSA boundaries and audit checkpoint signatures. |
| Air-gap and power state | `mai-core/src/airgap/`, `mai-core/src/power/` | Complete at software layer | Hardware switch monitoring remains deployment scoped. |
| Lamprey router | `mai-router/src/` | Complete | Sensitivity classifier, entity detector, programmable rules. |
| Compliance engines | `mai-compliance/src/` | Complete | HIPAA, ITAR/EAR, OCAP, policy composer. |
| Trust Manifold | `mai-compliance/src/{bundle,trust_cache,trust}.rs` | Complete | Signed claims, signed bundles, local trust cache. |
| Audit and reports | `mai-compliance/src/audit/`, `mai-compliance/src/reports/` | Complete | Hash chain, report certification, TrustSection. |
| Dashboard and demos | `compliance-dashboard/`, `apps/`, `deployment/` | Complete | FastAPI dashboard and runnable scenario scaffolds. |

**Current test posture:** Rust workspace lib tests exceed 1196; Python tests exceed 114; SDK, dashboard, and scaffold tests are green in their respective suites. SHIP-07 convergence adds 4 integration + 5 unit tests in mai-api; SHIP-04 fixup adds 7 audit_wal integration tests. Hardware-dependent Phase 1 exit criteria remain explicit deferrals rather than hidden gaps.

The full session-by-session build history -- deliverables, acceptance criteria, and deviation notes -- lives in [SESSION-LOG.md](sessions/SESSION-LOG.md) and the archive files above. That history is reference material. Use it when you need provenance; do not make new reviewers start there.

---

## Five Things That Will Bite You If You Ignore Them

**1. The HIL is the moat.** The hardware interface layer survives hardware generations. The TetraMem MX100 arrives in 2028. The HIL is what makes that transition painless. Cut corners here and you become a one-generation company.

**2. Adapters are disposable. The core is not.** Backend adapters can be rewritten when Ollama's API changes or vLLM ships v2. The core kernel and API surface cannot be rewritten without breaking every application above them.

**3. Air-gap is not a checkbox.** It is an architectural constraint affecting every component. Every component must work with zero network access. If you find yourself writing `if air_gap_mode:` conditionals, you have already failed. The default is air-gapped. Network is the exception.

**4. PQC is ahead of schedule, on purpose.** ML-KEM and ML-DSA deployment in 2026 puts Island Mountain ahead of the NIST 2030 deadline by four years. This is a competitive advantage, not decorative crypto.

**5. The quantum memristor transition is not science fiction.** TetraMem has shipping eval hardware. The HIL and adapter framework are designed so a TetraMem adapter slots in without changing core kernel or application code. If your implementation violates that property, you have failed the most important test.

---

## Critical Path Remaining

The mainline build plan (S1..S46) is complete. The remaining critical path
lives in the ship-hardening lane and the deployment ladder.

Parallel tracks:

- Track A: Scheduler -- complete.
- Track B: Security -- complete at policy/software layer; hardware-only enforcement remains deployment scoped.
- Track C: Applications -- six plan-spec scaffolds shipped; Part 2 family-app scaffolds optional under the plan.
- Track D: Power and Lifecycle -- complete.
- Track L: Lamprey Compliance -- complete.
- Track H2: Trust Manifold backfills -- complete, lane closed 2026-05-22.
- Track SHIP (hardening): complete through SHIP-17. Production-readiness endpoint, standalone `mai-ship-validate`, packaging, backup/restore, observability, release gates, burn-in tooling, operator docs, final audit pass, and auth-bypass consistency guard have landed.
- Track RC / DOUGHERTY: RC1 tester bundle shipped; John Dougherty's outside-tester review opened the active DOUGHERTY remediation lane. Finish remaining J sessions, then RC-11 re-bundle/re-ship. Roadmap in `docs/COGENT-DEPLOYMENT-ROADMAP.md`.

See `BUILD-EXECUTION-PLAN-V2-UPDATED.md` for the governing execution plan, [MAI-BUILD-PROMPT-ROSTER-v2.md](../MAI-BUILD-PROMPT-ROSTER-v2.md) for per-session deliverables, and [SHIP-HARDENING-PLAN.md](sessions/SHIP-HARDENING-PLAN.md) for the hardening sequence.

---

## Detailed Build History

This section intentionally preserves the operational memory that keeps the file from becoming a pretty but shallow orientation page. It is summarized from the active session log and roster; if a detail matters for implementation, verify it in the linked source file before changing code.

### Foundation And Integration

**Sessions 01-05: Specification.** The first phase produced the master architecture, HIL specification, adapter framework specification, core kernel specification, and API surface specification. These documents established the Tock-inspired trust boundary: a trusted Rust core with untrusted adapters behind a stable API.

**Session 06: Project scaffold and HIL.** The monorepo scaffold landed with `mai-core`, `mai-hil`, `mai-adapters`, `mai-api`, SDK crates, Python adapters, configs, tests, and docs. HIL traits established typed surfaces for hardware probing, memory management, power states, secure load, and adapter integration.

**Session 07: Core kernel.** Registry, scheduler, power, health, and hotswap foundations landed in Rust. These modules remain trusted and should not drift toward adapter-specific behavior.

**Sessions 08-09: Adapter framework and backend adapters.** The adapter manager, Python runner, and backend implementations covered Ollama, vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2, and SGLang. Adapter work is intentionally disposable compared with the core API and scheduler contract.

**Session 10 and 10d: Integration testing and response cache.** Integration tests and benchmark scaffolding arrived, followed by a standalone `mai-core/src/cache.rs` response cache with TTL, LRU eviction, memory budget enforcement, profile isolation, and BLAKE3 key hashing. Cache integration into later scheduler/hotswap flows remains governed by the surrounding modules.

**Sessions 11-14c: API server and real inference path.** The REST/gRPC API, streaming surfaces, auth middleware, audit middleware, SDK compatibility routes, first-boot admin key, rate limiting, and real adapter-backed inference path landed. By Session 14b, HTTP requests produced real tokens from real adapters and no placeholder inference content remained.

**Session 12: Vault integration.** The `mai-vault` crate and expanded `mai-core/vault.rs` created the L2 vault boundary: model storage, PQC provider, TPM provider, profile store, audit store, and vector store traits. PQC and ZFS linking remain deployment concerns, but the trust shapes are in place.

**Session 13: Agent/RAG interface.** The `mai-agent` crate added context management, tool registry, RAG pipeline interface, STT manager, and agentic task lifecycle. Full L4 application behavior remains outside the MAI core scope, but the integration surface is present.

### Scheduler, Power, And Simulation

**Session 15: Scheduler core architecture.** `mai-scheduler` introduced the object-safe `Scheduler` trait, `DefaultScheduler`, `InstanceRegistry`, `PlacementEngine`, and `AliasResolver`. It integrated into REST, gRPC, and SSE inference paths and became the core placement contract.

**Session 16: GPU topology.** Topology discovery parsed GPU inventory and interconnect information into weighted graphs, best pairs/quads, NVLink cliques, path costs, and CPU affinity groupings. The topology graph is intentionally abstract so future HIL drivers can feed non-CUDA fabrics.

**Session 17: KV cache manager.** KV cache management added sequence tracking, memory accounting, eviction scoring, anti-thrashing, priority immunity for system requests, and proactive/standard/emergency trigger thresholds.

**Session 18: Continuous batching.** Per-instance batch builders, admission control, emergency preemption, wait-time metrics, and KV eviction protection landed. Batching is part of placement, not an adapter-side afterthought.

**Session 19: Multi-factor scorer.** The scoring module combined latency, memory pressure, topology penalty, eviction cost, batching benefit, and continuation routing. API startup now loads scheduler, topology, KV, and scoring config before publishing the scheduler.

**Session 20: Metrics feedback loop.** Scheduler metrics added lifecycle tracking, completion feedback, health scoring, anomaly detection, and ring-buffer storage. `mai-api/src/handlers/telemetry.rs` exposes the surface.

**Session 21: Simulation framework.** `tools/simulator/` added a discrete-event engine, GPU model, workload generator, KV policies, metrics, reporting, experiment runner, and tuning config. The simulator is the offline policy laboratory.

**Sessions 22-23: Power and Sentinel.** Power state machine refactors and Sentinel mode landed. Sentinel estimator/runtime/promotion/warmup modules preserve low-power inference readiness while making promotion explicit.

**Session 25: OTA and model lifecycle.** Update transport boundaries, manifests, differential shard planning, resumable download, lifecycle listing, load/unload, benchmark, export, affinity tracking, and preload planning landed. The update protocol doc explains the privacy-preserving mirror contract.

### Security, Air-Gap, And Gate C

**Session 26: Auth hardening.** API key generation moved to OS randomness, API key validation and rate limits were hardened, the Rust SDK gained auth config helpers, and acceptance tests covered missing/invalid/valid/rate-limited/spoofed/exempt health cases.

**Session 27: Vault crypto.** PQC-backed package verification/encryption paths, AEAD TPM sealing, audit checkpoint signatures, and first-boot orchestration replaced earlier stubs.

**Session 28 and BF-4: Air-gap and local trust cache.** A canonical `ConnectivityState` and shared `AirGapPolicy` landed. Loopback/wildcard bind enforcement, `/v1/system/airgap`, and trust-cache freshness states created the software air-gap policy floor. Hardware switch monitoring remains production-deployment scoped.

**Session 32: Production trace integration.** Opt-in NDJSON trace capture, anonymization, reconstruction, calibration, hybrid workloads, replay comparison, and Markdown/JSON reporting landed. Privacy is structural: captures and anonymizers use documented allowlists, and tests assert no prompt/response text leaks.

**Session 33: Cross-GPU scheduling and soft eviction.** Soft eviction, tiered KV control, preemption hierarchy, cross-instance migration scoring, and decision cache primitives were added to `mai-scheduler`.

**Session 34: Integration validation.** System integration tests closed air-gap, power transition, profile permission, and zero-data-leak gaps. Integration coverage documentation maps each coverage area to test files.

**Session 35: Deployment packaging.** Launch scripts, health checks, burn-in scripts, stdlib-only smoke client, deployment guide, and known-issues refresh closed Gate C. Hardware-dependent criteria generate explicit deferral artifacts.

### Lamprey Compliance Governance

**Session 36: Query router.** `mai-router` added sensitivity classification, entity detection, cloud/local routing, budget handling, fallback, and deterministic route decisions with audit-grade reasons.

**Session 37: Programmable policy rules.** Rule engine, module registry, TOML rule loading, and rule tester CLI landed. Baseline HIPAA, ITAR, and OCAP rule modules ship under `mai-router/rules-config/`.

**Session 38: HIPAA.** The HIPAA engine detects all 18 Safe Harbor identifiers, aggregates PHI reports without retaining raw matched text, enforces BAA modes, supports de-identification, and enriches routing with medical entities.

**Session 39: ITAR/EAR.** Export-control routing added controlled-technical-data indicators, jurisdiction-aware backend eligibility, and trust-claim access checks.

**Session 40: OCAP.** OCAP evaluation landed with possession, control, sacred/elder role gates, cultural consent, treaty consent, trust context, and local-only enforcement. This is a core differentiator, not a decorative module.

**Session 41: Policy runtime.** The composer resolves HIPAA, ITAR/EAR, and OCAP decisions with deny-wins behavior, most-restrictive-route, priority-ordered reasons, decision caching, policy templates, and management APIs.

**Session 42 and BF-5: Audit chain and correlation.** The compliance audit log added BLAKE3 hash-chain storage, optional ML-DSA periodic signatures, JSON-lines WAL, and correlation fields linking credential events, Lamprey decisions, and MAI requests.

**Session 43: Compliance reports.** Built-in HIPAA, ITAR, OCAP, SystemActivity, and MonthlyDigest reports landed with JSON/HTML/CSV/Text output and report certification. Every report carries a `TrustSection`.

**Session 44 and BF-6: Dashboard and trust APIs.** `mai-api` serves trust status, claims, bundle status, revocation status, token exchange, compliance policy, audit, reports, and SSE feed endpoints. The Python SDK exposes `client.trust.*` and `client.compliance.*`; the FastAPI dashboard exposes overview, audit, reports, policy, alerts, and health pages.

**BF-7: Acquisition and demo backfill.** `ACQUISITION-PACKAGE.md`, `BUYER-INTEGRATION-GUIDE.md`, and `DEMO-SUITE.md` became the acquisition/diligence seed docs. The OpenBao Trust Demo and operator scaffold regressions were repaired. Trust Manifold backfills are closed.

### Application Scaffolds

Six reference scaffolds live under `apps/`:

- `local-secure-inference`: authenticated local chat and streaming path.
- `rag-reference`: ingest, embed, retrieve, and answer flow.
- `compliance-routed`: policy-shape demo for routed decisions.
- `tribal-sovereignty`: OCAP local-only route and model guard demo.
- `operator`: plain-text/JSON local instance status dashboard.
- `openbao-trust-demo`: Trust Manifold claim, cache, token, inference, and audit summary flow.

Each app ships a `README.md`, `config.toml`, `main.py`, and smoke/integration tests. They are demo scaffolds, not full L5 products.

### CI And Known Drift

Three classes of build drift have already been repaired:

- Pytest collection failures and adapter constructor mismatch.
- `cargo fmt` formatting drift across multiple crates.
- Test callsite type mismatches after production signatures changed.

If these reappear, fix production code or test callsites deliberately. Do not paper over them with broad ignores. `docs/KNOWN-ISSUES.md` is the place for explicit deferrals.

---

## What Is Not In Scope

These items are explicitly excluded. See [KNOWN-ISSUES.md](KNOWN-ISSUES.md) for the full list.

- L6 UI beyond the thin compliance dashboard.
- Remote support tunnel.
- Landfall Council multi-user chat variant.
- Full L4 agent logic.
- Full L5 application logic.
- TetraMem production adapter.
- Photonic adapter.
- Hardware switch enforcement beyond the deployment-scoped production session.

---

## Production Readiness Checklist

Every item must pass before MAI ships on hardware:

- [x] Full workspace test suite passes (S46 + SHIP-07-convergence; 1196+ Rust lib + ~108 mai-api integration + Python suites).
- [x] Ship-profile production guard fails closed at startup before any socket binds (SHIP-07-convergence; `ProductionReadinessReport::evaluate_with_runtime` inside `MaiServer::run`).
- [x] Demo-safe defaults (`StubVault`, `MemoryAuditWriter`, `NullSealer`, `AcceptAllBundleVerifier`) unreachable in production startup when `MAI_SHIP_PROFILE` is set (SHIP-07-convergence).
- [x] `mai-ship-validate` standalone binary + `GET /v1/system/production-readiness` admin endpoint (SHIP-07-endpoint-and-cli, commit `7b746c0`).
- [x] Backup + restore tooling (SHIP-09 `mai-admin backup create/verify` commit `7b746c0`; SHIP-10 `mai-admin restore plan/apply` + DR drills commit `0fe5f59`).
- [ ] Persistent state recovery drill on representative Scout / Ranger hardware (carried to SHIP-14 72-hour burn-in).
- [ ] 72-hour burn-in passes on representative Scout hardware.
- [ ] 72-hour burn-in passes on representative Ranger hardware.
- [ ] Air-gap verification passes 72-hour endurance.
- [ ] PQC encryption verified on all vault data.
- [ ] Audit trail integrity verified over 72-hour period (SHIP-04 acceptance suite covers tamper detection + replay + rotation today; the 72-hour endurance signoff lands in SHIP-14).
- [ ] First boot completes in under 3 minutes.
- [ ] Model update via USB verified.
- [ ] All 7 adapters health-check pass.
- [ ] Power state transitions verified on hardware.
- [ ] Scheduler topology correctly maps hardware.
- [ ] Documentation reviewed and complete.
- [ ] Performance baseline stored for future regression detection.

---

## Related Documents

- [ARCHITECTURE.md](../ARCHITECTURE.md): System architecture and trust model.
- [CONVENTIONS.md](../CONVENTIONS.md): Coding standards and naming rules.
- [PROJECT.md](../PROJECT.md): Scope, phases, timeline.
- [SESSION-RULES.md](../SESSION-RULES.md): Session governance and quality gates.
- [SESSION-LOG.md](sessions/SESSION-LOG.md): Session progress tracking.
- [KNOWN-ISSUES.md](KNOWN-ISSUES.md): Limitations and deferred items.
- [INDEX.md](INDEX.md): Master file index.
- [MAI-BUILD-PROMPT-ROSTER-v2.md](../MAI-BUILD-PROMPT-ROSTER-v2.md): Complete session prompts and deliverables.
- [MAI-BUILD-PROMPT-ROSTER.md](../MAI-BUILD-PROMPT-ROSTER.md): Original session prompts.

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*
