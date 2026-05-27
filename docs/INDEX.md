# MAI Document Index

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Last Updated:** 2026-05-26 (post-RC1.2 governance sweep, see [LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md](LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md))

> **Current build state (2026-05-26)**
> - **Mainline (Sessions 1-46 + BF-1..BF-7): CLOSED** — Gate D shipped at commit `22f0f66`.
> - **Ship hardening (SHIP-01..SHIP-17): CLOSED 2026-05-23** — final hotfix at `dceaabc`.
> - **RC1 release (RC-01..RC-10): CLOSED 2026-05-24** — RC1.0 to John Dougherty (`e2d9ea6`).
> - **DOUGHERTY remediation lane (J-01..J-26): CLOSED 2026-05-25** — J-23..J-26 landed under `a072634`; closure doc at [`dougherty/J-15-DOUGHERTY-CLOSURE.md`](dougherty/J-15-DOUGHERTY-CLOSURE.md). All 26 sessions complete.
> - **RC1.2 re-ship (RC-11): COMPLETE 2026-05-25** — freeze `e55c1ff`, local GitDoctor score **93/100** at freeze; bundle assembled, ready for tester re-scan.
> - **GITDOCTOR-75 lane (GD75-01..GD75-16): IN-FLIGHT** — GD75-07/08/09/10/14/15 landed (`24a6700`, `a121d4a`, `28f0386`, `23b876c`); target external rescan ≥95/100. See [`GITDOCTOR-75-REMEDIATION-PLAN.md`](GITDOCTOR-75-REMEDIATION-PLAN.md).
> - **IGD lane (internal-scan follow-up, loose): IN-FLIGHT** — IGD-01/04/05/08 merged today (`ae30614`, `ad8420a`, `330185a`, `a6f3ffc`); fixes gitleaks allowlist + deployment-staging gitignore, clippy collapsible_if, commit-msg hook + CI co-author enforcement, trailing newlines + .editorconfig. No formal plan doc; traceable via `git log --grep IGD-`.
> - **Recent today (2026-05-26):** SEC-1 (GitHub secret-scan alert #1 — Vault token leak — closed/revoked; superseded by parallel commits `c75e95f`+`c8055ea`); HF-2 (`c108ea0`, re-add tracing::warn import); WARN-1, CLIP-1, GOV-1 (this sweep).
> - **Latest local scan (2026-05-26, GOV-1):** [LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md](LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md). Layer 1 mapped: **90/100** (52/58 — down 3 points vs Memorial Day 93/100 because the SEC-1 env-var refactor introduced CFG-001 hardcoded-localhost fallback in `openbao_client.rs` and REV-005 HIGH `.unwrap_or_default()` at `openbao_client.rs:340`). Layer 2 independent tools: 5 FAIL (cargo test workspace, cargo audit `pyo3 0.22.6` advisory + `proc-macro-error` unmaintained, cargo deny `idna 0.5`, pip-audit pip 26.0.1 CVE-2026-3219/6357, detect-secrets keyword in `mai-sdk-python/docs/authentication.md`). Layer 3 adversarial: PASS. Address these in the GITDOCTOR-75 / IGD lanes.
> - **Next gate:** RC2 deployment rehearsal (RC2-01..RC2-08). Production-validation evidence already committed at `ee6eb13`. See [`RC2-SESSION-PLAN.md`](RC2-SESSION-PLAN.md).
>
> Docs marked **STATUS — CLOSED** / **STATUS — SUPERSEDED** at the top are kept for historical reference and do not drive active work.

---

## Start Here by Role

| If you are... | Go to |
|---|---|
| Reviewing the product thesis | [ACQUISITION-PACKAGE.md](ACQUISITION-PACKAGE.md) |
| Running the demo suite | [DEMO-SUITE.md](DEMO-SUITE.md) |
| Reviewing integration architecture | [BUYER-INTEGRATION-GUIDE.md](BUYER-INTEGRATION-GUIDE.md) |
| Reviewing the scheduler design | [SCHEDULER-BRIEF.md](SCHEDULER-BRIEF.md) |
| Reviewing the compliance governance stack | [LAMPREY-BRIEF.md](LAMPREY-BRIEF.md) |
| Reviewing the air-gap security model | [AIR-GAP-BRIEF.md](AIR-GAP-BRIEF.md) |
| Running the acquisition demo scenarios | [acquisition/demos/](acquisition/demos/) |
| Building against the Python SDK | [SDK-REFERENCE.md](SDK-REFERENCE.md) |
| Calling the REST API directly | [API-REFERENCE.md](API-REFERENCE.md) |
| Operating a local node | [DEPLOYMENT.md](DEPLOYMENT.md) |
| Installing a production appliance | [INSTALL.md](INSTALL.md) |
| Running an installed appliance day-to-day | [OPERATIONS.md](OPERATIONS.md) |
| Handling an outage or alert at 2 AM | [runbooks/](runbooks/) and [INCIDENT-RESPONSE.md](INCIDENT-RESPONSE.md) |
| Upgrading or rolling back a release | [UPGRADE-ROLLBACK.md](UPGRADE-ROLLBACK.md) |
| Backing up or restoring a node | [BACKUP-RESTORE.md](BACKUP-RESTORE.md) |
| Planning tester packaging and production shipping | [COGENT-DEPLOYMENT-ROADMAP.md](COGENT-DEPLOYMENT-ROADMAP.md) |
| Planning copyright, patent, and IP protection | [IP-COPYRIGHT-PATENT-ROADMAP.md](IP-COPYRIGHT-PATENT-ROADMAP.md) |
| Starting as a new engineer | [HANDOFF.md](HANDOFF.md) |
| Troubleshooting a first-run error | [KNOWN-ISSUES.md](KNOWN-ISSUES.md) |

---

## External Review and Acquisition Documents

These documents are written for buyers, security architects, integration
engineers, and technical reviewers. Start here if you are not on the
build team.

| File | Purpose | Read When |
|---|---|---|
| [ACQUISITION-PACKAGE.md](ACQUISITION-PACKAGE.md) | Five-point buyer thesis with code and test citations | Acquirer diligence; investor narrative |
| [BUYER-INTEGRATION-GUIDE.md](BUYER-INTEGRATION-GUIDE.md) | OpenBao-backed trust boundary, 7-step integration sequence, boundary-review checklist | Acquirer integration engineering; security architecture review |
| [DEMO-SUITE.md](DEMO-SUITE.md) | Trust Manifold 8-step scenario, supporting demos, reproducibility checklist | Acquirer technical review; sales engineering |
| [SCHEDULER-BRIEF.md](SCHEDULER-BRIEF.md) | Scheduler technical brief: topology, KV, batching, scoring, balancer, decision cache, power, trace replay | Scheduling-architect diligence |
| [LAMPREY-BRIEF.md](LAMPREY-BRIEF.md) | Lamprey three-layer governance stack: router, policy, audit; module and composer reference | Compliance and governance diligence |
| [AIR-GAP-BRIEF.md](AIR-GAP-BRIEF.md) | Air-gap as routing input: ConnectivityState, loopback bind, trust-cache interaction, audit coverage | Security and network policy reviewers |
| [API-REFERENCE.md](API-REFERENCE.md) | Live REST surface: inference, models, health, system, scheduler telemetry, trust, compliance | Integration engineers, SDK authors |
| [SDK-REFERENCE.md](SDK-REFERENCE.md) | Python SDK namespace reference: client.models / chat / scheduler / trust / compliance / auth, errors, CLI | Application developers, embed teams |
| [acquisition/ARCHITECTURE.md](acquisition/ARCHITECTURE.md) | Top-down architecture overlay: three-layer Lamprey, MAI, Trust Manifold; integration shapes A/B/C | Acquirer architecture review |
| [acquisition/COMPETITIVE.md](acquisition/COMPETITIVE.md) | Competitive analysis vs Guardrails AI, NeMo Guardrails, Minder, Cloudflare AI Gateway, AWS Bedrock, Azure | M&A analysts, product strategy |
| [acquisition/IP.md](acquisition/IP.md) | IP position memo: 4 patent candidates, trade secrets, open-source boundary recommendations (not legal advice) | IP counsel, corp dev |
| [acquisition/INTEGRATION.md](acquisition/INTEGRATION.md) | Acquirer integration guide: custom modules, SIEM bridge, config semantics, build and test surface | Acquirer engineering embed team |
| [acquisition/demos/healthcare.md](acquisition/demos/healthcare.md) | Demo 1 -- HIPAA scenario walkthrough | Acquirer technical reviewer |
| [acquisition/demos/defense.md](acquisition/demos/defense.md) | Demo 2 -- ITAR/EAR scenario walkthrough | Acquirer technical reviewer |
| [acquisition/demos/tribal.md](acquisition/demos/tribal.md) | Demo 3 -- OCAP tribal sovereignty walkthrough with all 9 pipeline stages | Acquirer technical reviewer; tribal data governance |
| [acquisition/demos/multi-domain.md](acquisition/demos/multi-domain.md) | Demo 4 -- multi-module conflict resolution (HIPAA + OCAP), composer fold rules, precedence chain | Acquirer technical reviewer |
| [acquisition/READY.md](acquisition/READY.md) | Gate D production readiness -- test, demo, and perf evidence; known issues; certification statement | Acquirer reviewer; release sign-off |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Out-of-scope items, deferred work, architectural limitations, open questions | Troubleshooting; "should I build this?" |

---

## Operator Production Docs (SHIP-15)

Production-only operator docs for `profile.mode = "production"`
appliances. The runbook index in
[`runbooks/README.md`](runbooks/README.md) maps every alert
class to a specific named-failure procedure.

| File | Purpose | Read When |
|---|---|---|
| [INSTALL.md](INSTALL.md) | Operator install procedure: hardware/software prereqs, package install, validator gate, first-backup floor | Bringing up a new appliance |
| [FIRST-BOOT.md](FIRST-BOOT.md) | The privileged first-boot key-mint contract; why there is no recovery for a lost first-boot key | Reading before runbook 01 |
| [OPERATIONS.md](OPERATIONS.md) | Day-2 cadence: daily/weekly/quarterly/annual operator tasks; endpoint cheat sheet; config touchpoints | After install completes |
| [BACKUP-RESTORE.md](BACKUP-RESTORE.md) | Backup component manifest, signing key custody, retention contract, restore plan/apply, DR drill matrix | Building backup policy or restoring a node |
| [OBSERVABILITY.md](OBSERVABILITY.md) | Health surfaces, Prometheus metrics, alert rule map, dashboard scope, payload boundary | Wiring monitoring or paging |
| [RELEASE-GATES.md](RELEASE-GATES.md) | `mai-ship-validate` exit codes, check families, pre-release sequence (dev/package/installed/recovery/burn-in) | Declaring a build shippable |
| [SECURITY-PRODUCTION.md](SECURITY-PRODUCTION.md) | What ship promises, four-key custody matrix, rotation cadence, reverse-proxy contract, in-process egress | Reviewing production security posture |
| [TRUST-BRIDGE-PRODUCTION.md](TRUST-BRIDGE-PRODUCTION.md) | Roles for bundle authoring/signing/delivery; why the bridge is procedural; anchor distribution; revocation | Setting up signed-bundle delivery |
| [AUDIT-RETENTION.md](AUDIT-RETENTION.md) | Two-chain WAL contract, default retention tiers, export procedure for counsel, cold-storage archival | Building retention policy or handling counsel requests |
| [UPGRADE-ROLLBACK.md](UPGRADE-ROLLBACK.md) | Pre-upgrade checklist, upgrade procedure, multi-release rolling, rollback to verified backup | Planning or executing an upgrade |
| [INCIDENT-RESPONSE.md](INCIDENT-RESPONSE.md) | Severity classes, first-five-minutes, investigation flow, post-mortem template, specific incident shapes | At 2 AM, when something is wrong |
| [runbooks/README.md](runbooks/README.md) | Index of 14 named-failure runbooks (key rotation, anchor rotation, bundle import, audit verify, compliance report, backup, restore, upgrade rollback, adapter crash loop, trust bundle expired, audit WAL tamper, air-gap violation, disk almost full) | When an alert fires |

---

## Internal Governance Documents

These documents govern the MAI build. They are written for engineers
on the project. Read them before writing code.

| File | Purpose | Read When |
|---|---|---|
| [HANDOFF.md](HANDOFF.md) | Founding engineer orientation, critical warnings, current state, what will bite you | First day on the project |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Tock-to-MAI architecture map, trust boundaries, component catalog, data flows, power state machine | Understanding system structure |
| [CONVENTIONS.md](CONVENTIONS.md) | Language assignments, code quality gates, monorepo layout, testing rules, git conventions | Writing any code |
| [SESSION-RULES.md](SESSION-RULES.md) | Dependency enforcement, acceptance criteria protocol, quality gates, session workflow | Conducting any session |
| [IPC-PROTOCOL.md](IPC-PROTOCOL.md) | NDJSON IPC wire format spec for Rust-Python adapter communication | Working on adapter IPC |
| [SESSION-LOG.md](SESSION-LOG.md) | Session progress for Phase H through Gate D scope (Sessions 26-46 + BF-1..BF-7) | Before and after each session |
| [SESSION-LOG-ARCHIVE-01.md](SESSION-LOG-ARCHIVE-01.md) | Completed sessions 01-10 (Phases A+B) | Reviewing past session details |
| [SESSION-LOG-ARCHIVE-02.md](SESSION-LOG-ARCHIVE-02.md) | Completed sessions 11-25 (Phases C through G), archived 2026-05-23 | Reviewing past session details |
| [SESSION-LOG-ARCHIVE-03.md](SESSION-LOG-ARCHIVE-03.md) | Completed sessions 26-46 plus BF-1..BF-7 (Security through Gate D), archived 2026-05-23 | Reviewing Gate D build history |
| [HANDOFF-ARCHIVE-01.md](HANDOFF-ARCHIVE-01.md) | Archived onboarding walkthrough and Phase A+B code inventory | Reference only |
| [MAI-BUILD-PROMPT-ROSTER-v2.md](MAI-BUILD-PROMPT-ROSTER-v2.md) | Complete session prompts, deliverables, and acceptance criteria for all 46 sessions | Starting any session |
| [PROJECT.md](PROJECT.md) | Original scope, 5-phase plan, 18-session timeline, effort estimates, coverage matrix | Historical scope reference |
| [SESSION-46-PLAN.md](SESSION-46-PLAN.md) | Session 46 plan: scope, file layout, test inventory, perf targets, Gate D checklist | Session 46 implementer |
| [COGENT-DEPLOYMENT-ROADMAP.md](COGENT-DEPLOYMENT-ROADMAP.md) | Session-by-session roadmap from Gate D codebase to RC1 tester bundle, hardened release candidate, and production appliance | Release planning; deployment hardening |
| [IP-COPYRIGHT-PATENT-ROADMAP.md](IP-COPYRIGHT-PATENT-ROADMAP.md) | Step-by-step owner-side process for copyright, patent, trade secret, licensing, disclosure, and release gates | IP planning; external release preparation |
| [INDEX.md](INDEX.md) | This file | Finding anything |

---

## Running the Compliance Demo Suite

The Session 46 compliance demo suite is implemented in
`mai-compliance/tests/compliance_demos.rs` (6 scenarios) and
`mai-compliance/tests/compliance_perf.rs` (3 perf baselines).

```powershell
cargo test -p mai-compliance --test compliance_demos
cargo test -p mai-compliance --test compliance_perf -- --nocapture
```

---

## Session-to-Document Map

Each session produces specific deliverables. This table maps sessions
to their primary output types.

### Phase A: Specification (Sessions 01-05)

| Session | Title | Primary Outputs |
|---|---|---|
| 01 | MAI Master Architecture Specification | Architecture doc (40-60 pages), project scaffold, dependency graph, glossary |
| 02 | Hardware Interface Layer (HIL) Specification | HIL trait defs (Rust), driver specs, power state machine diagram, air-gap daemon spec |
| 03 | Backend Adapter Framework Specification | Adapter traits (Rust + Python), per-backend specs (7 backends), lifecycle and sandboxing specs |
| 04 | MAI Core Kernel Specification | Scheduler spec, registry spec, health monitor spec, power state machine spec, hot-swap spec |
| 05 | MAI API Surface Specification | OpenAPI 3.1 YAML, Proto3 defs, Python SDK skeleton, Rust SDK skeleton, auth/error specs |

### Phase B: Foundation Code (Sessions 06-10)

| Session | Title | Primary Outputs |
|---|---|---|
| 06 | Project Scaffold + HIL Implementation | Full monorepo scaffold, NVIDIA/AMD/CPU drivers, power state controller, memory manager |
| 07 | MAI Core Kernel Implementation | registry.rs, scheduler.rs, power.rs, health.rs, hotswap.rs (production Rust) |
| 08 | Backend Adapter Framework + Ollama Adapter | AdapterManager (Rust), JSON-RPC IPC bridge, adapter runner (Python), Ollama adapter (full), 21 tests |
| 09 | Remaining Backend Adapters | vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2, SGLang adapters |
| 10 | End-to-End Integration Testing | 14 integration tests, benchmark suite (8 metrics), CI pipeline |

### Phase C: Integration Code (Sessions 11-13)

| Session | Title | Primary Outputs |
|---|---|---|
| 11 | MAI API Server Implementation | REST (axum), gRPC (tonic), SSE/WebSocket streaming, auth middleware, audit middleware |
| 12 | Vault Integration (L2 Interface) | ZFS vault interface, PQC encryption (ML-KEM/ML-DSA), profile store, audit trail, Qdrant |
| 13 | Agent/RAG Interface (L4 Integration) | Context management, RAG pipeline interface, tool calling, speech-to-text, agentic tasks |

### Phase D-Prep: Wiring Sprint (Sessions 14a-14c)

| Session | Title | Primary Outputs |
|---|---|---|
| 14a | Adapter IPC Contract + NDJSON Protocol | IPC-PROTOCOL.md, bridge.rs, process.rs, manager.rs, runner.py, test_ipc_protocol.py |
| 14b | Real Inference Path Wiring | AdapterManager in server.rs, real adapter calls in inference.rs, SSE streaming, e2e_inference.sh |
| 14c | API/SDK Route Alignment + Auth | Auth hardening, rate limiting, SDK streaming, first-boot admin key, BUILD.md |

### Phase D: Scheduler Foundation (Sessions 15-18, 24)

| Session | Title | Primary Outputs |
|---|---|---|
| 15 | Scheduler Core Architecture | mai-scheduler crate (7 files, 41+ tests), Scheduler trait, DefaultScheduler, API integration |
| 16 | GPU Topology Discovery + Weighted Graph | topology module (5 files, 41 unit tests), integration tests (16 tests), fixtures, config/topology.toml |
| 17 | KV Cache Manager | kv/ module (6 files, 53 unit tests + 5 integration tests), KvCacheManager trait, HeuristicKvCacheManager, config/kv.toml |
| 18 | Continuous Batching Engine | batch/ module (5 files, 52 tests), BatchBuilder, AdmissionController, PreemptionPolicy, BatchMetrics, eviction batch_contribution wired |
| 24 | Integration Seam Fixes | Model install/remove pipeline refactoring, axum version conflict workaround, ProfileRole/ProfilePermissions alignment |

### Phase E: Scheduler Intelligence (Sessions 19-21)

| Session | Title | Primary Outputs |
|---|---|---|
| 19 | Multi-Factor Scorer | scoring/ module, ScoringConfig, config/scoring.toml, API startup autoload, topology/KV handles, score breakdown, Session 19f schedule tests |
| 20 | Feedback Loop + Metrics Collection | metrics/ module, MetricsCollector, lifecycle/feedback/health/anomaly/store, telemetry handler, config/metrics.toml |
| 21 | Simulation Framework | tools/simulator engine, GPU model, workload generator, KV policies, metrics, experiments, config, README |

### Phase F: Power and Lifecycle (Sessions 22-25)

| Session | Title | Primary Outputs |
|---|---|---|
| 22 | Power State Machine | mai-core power refactor, scheduler-facing power controller, config/power.toml |
| 23 | Sentinel Mode | Sentinel estimator/runtime/promotion/warmup modules, config/sentinel.toml |
| 24 | Model Install/Remove Seams | Install/remove pipeline refactor, route conflict fix, required_vram_bytes propagation |
| 25 | OTA Update Pipeline + Model Lifecycle | update/lifecycle/preload modules, benchmark/update REST routes, docs/UPDATE-PROTOCOL.md |

### Phase L: Compliance Governance (Sessions 36-46)

| Layer | Sessions | Primary Outputs |
|---|---|---|
| L1 Router | 36-37 | Query router, sensitivity/entity detection, programmable routing policies |
| L2 Policy | 38-41 | HIPAA, ITAR/EAR, OCAP, conflict resolution, compliance policy runtime |
| L3 Audit | 42-44 | Compliance audit log, report generation, management dashboard/API |
| Acquisition Prep | 45-46 | Documentation package, demos, end-to-end compliance integration tests |

---

## Crate and Package Index

| Crate/Package | Language | Trust Level | Session Created | Session Implemented |
|---|---|---|---|---|
| mai-core | Rust | Trusted | 06 (scaffold) | 07 |
| mai-hil | Rust | Trusted | 06 (scaffold) | 06 |
| mai-adapters | Rust + PyO3 | Trusted (framework) | 06 (scaffold) | 08 |
| mai-api | Rust | Trusted | 06 (scaffold) | 11 |
| mai-sdk-python | Python | N/A (SDK) | 06 (scaffold) | 05 (skeleton), 11 (full) |
| mai-sdk-rs | Rust | N/A (SDK) | 06 (scaffold) | 05 (skeleton), 11 (full) |
| mai-vault | Rust | Trusted | 12 | 12 |
| mai-agent | Rust | Trusted (L3-L4 boundary) | 13 | 13 |
| mai-scheduler | Rust | Trusted | 15 | 15 |
| tools/simulator | Python | Dev/test tooling | 21 | 21 |
| adapters/ollama | Python | Untrusted | 06 (scaffold) | 08 |
| adapters/vllm | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/llamacpp | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tgi | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tensorrt | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/exllamav2 | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/sglang | Python | Untrusted | 06 (scaffold) | 09 |

---

## Key Module Index

| Module/File | Purpose | Session |
|---|---|---|
| `mai-scheduler/src/scoring/` | Multi-factor scheduler scoring: latency, memory, topology, eviction, batching | 19 |
| `mai-scheduler/src/metrics/` | Feedback loop, lifecycle tracking, health scoring, anomaly detection, ring buffers | 20 |
| `mai-api/src/handlers/telemetry.rs` | REST telemetry endpoints for scheduler metrics, instance metrics, health, anomalies | 20 |
| `tools/simulator/` | Offline simulator for scheduling, KV, batching, workload, and policy experiments | 21 |
| `mai-core/src/power/` | Power state machine refactor, transitions, demotion logic | 22 |
| `mai-scheduler/src/power.rs` | Scheduler-facing power controller | 22 |
| `mai-core/src/sentinel/` | Sentinel estimator, runtime, promotion, and warmup path | 23 |
| `mai-core/src/models/update.rs` | OTA manifest client boundary, differential shard planning, license/tier validation, resumable downloads | 25 |
| `mai-core/src/models/lifecycle.rs` | Installed model listing, load/unload, benchmark, export, affinity tracking | 25 |
| `mai-core/src/models/preload.rs` | Sentinel-first and affinity-based preload planning | 25 |
| `mai-api/src/handlers/updates.rs` | REST update check, background download start, and status endpoints | 25 |
| `docs/UPDATE-PROTOCOL.md` | Privacy-preserving update server and mirror protocol | 25 |

---

## Test Suites Index

| Suite | Location | Purpose | Session |
|---|---|---|---|
| Unit tests (Rust) | Per-crate `#[cfg(test)]` | Module-level correctness | 06-09, 11-15 |
| Unit tests (Python) | Per-adapter `tests/` | Adapter correctness with mocked backends | 08-09 |
| mai-core lifecycle | `mai-core/tests/integration_lifecycle.rs` | Scheduler + health + power + hotswap (4 tests) | 07 |
| mai-adapters framework | `mai-adapters/tests/integration_adapters.rs` | Manager lifecycle, heartbeat, errors (4 tests + 2 benchmarks) | 08 |
| Benchmark suite | `mai-adapters/tests/benchmarks.rs` | 8 performance measurements (throughput, TTFT, overhead, memory, scaling, wake, load, swap) | 10 |
| Benchmark comparison | `tests/benchmarks/bench_compare.py` | Result storage, cross-run comparison, regression detection | 10 |
| Response cache unit tests | `mai-core/src/cache.rs` `#[cfg(test)]` | Cache hit/miss, TTL, eviction, profile isolation (12 tests) | 10d |
| Session 11a unit tests | `mai-api/src/{errors,config,auth,audit,air_gap}.rs` `#[cfg(test)]` | API errors, config loading, profile auth, audit chain, air-gap verify (45 tests) | 11a |
| Session 11c streaming tests | `mai-api/src/streaming/{mod,sse,ws}.rs` `#[cfg(test)]` | Token channel, backpressure, SSE format, WebSocket messages, auth handshake (31 tests) | 11c |
| Session 11d gRPC tests | `mai-api/src/grpc/{mod,health,power}.rs` `#[cfg(test)]` | Profile extraction, permission checks, error mapping, health utils, power transitions (18 tests) | 11d |
| Session 11e server tests | `mai-api/src/server.rs` `#[cfg(test)]` | Server config, stub vault, error display (4 tests) | 11e |
| HTTP integration | `mai-api/tests/http_integration.rs` | Chat, embeddings, models, admin, health, errors, guest (7 tests) | 11e |
| gRPC integration | `mai-api/tests/grpc_integration.rs` | Health, models, chat, auth rejection (4 tests) | 11e |
| Streaming integration | `mai-api/tests/streaming_integration.rs` | SSE events, heartbeat, done, 50-concurrent, non-streaming (5 tests) | 11e |
| Session 12 ZFS vault tests | `mai-vault/src/zfs.rs` `#[cfg(test)]` | Store, load, integrity, remove, snapshot lifecycle (7 tests) | 12 |
| Session 12 PQC tests | `mai-vault/src/pqc.rs` `#[cfg(test)]` | KEM/DSA keypair sizes, roundtrip, sign/verify, encrypt/decrypt, tamper detection (8 tests) | 12 |
| Session 12 TPM tests | `mai-vault/src/tpm.rs` `#[cfg(test)]` | Seal/unseal, PCR mismatch, recovery, key list/remove, attestation (5 tests) | 12 |
| Session 12 profile tests | `mai-vault/src/profiles.rs` `#[cfg(test)]` | CRUD, role filter, permissions, persistence, count (8 tests) | 12 |
| Session 12 audit tests | `mai-vault/src/audit.rs` `#[cfg(test)]` | Chain integrity, broken chain detection, profile/time queries, compliance export (7 tests) | 12 |
| Session 12 vector tests | `mai-vault/src/vectors.rs` `#[cfg(test)]` | Collection CRUD, similarity search, dimension validation, upsert, threshold filter (9 tests) | 12 |
| Session 13 context tests | `mai-agent/src/context.rs` `#[cfg(test)]` | Session lifecycle, truncation strategies, RAG/tool injection, token accounting (11 tests) | 13 |
| Session 13 tools tests | `mai-agent/src/tools.rs` `#[cfg(test)]` | Register/unregister, role filtering, chain lifecycle, parallel calls, audit trail (12 tests) | 13 |
| Session 13 RAG tests | `mai-agent/src/rag.rs` `#[cfg(test)]` | Cosine similarity, batch prep, packaging, cache hit/miss, profile isolation (13 tests) | 13 |
| Session 13 STT tests | `mai-agent/src/stt.rs` `#[cfg(test)]` | Transcription lifecycle, audio buffering, silence detection, format validation (10 tests) | 13 |
| Session 13 task tests | `mai-agent/src/tasks.rs` `#[cfg(test)]` | Submit/start/complete/cancel, budget exhaustion, concurrency, pruning (15 tests) | 13 |
| RAG pipeline integration | `mai-agent/tests/rag_pipeline_test.rs` | Full RAG flow, semantic cache, profile isolation, dimension validation (4 tests) | 13 |
| Tool calling integration | `mai-agent/tests/tool_calling_test.rs` | Chain round-trip, role access, parallel calls, step limits, model format (5 tests) | 13 |
| Task lifecycle integration | `mai-agent/tests/task_lifecycle_test.rs` | Full lifecycle, concurrency, budget exhaustion, cancel/fail, audit trail (7 tests) | 13 |
| IPC protocol tests | `adapters/tests/test_ipc_protocol.py` | NDJSON wire format contract verification (26 tests across 7 classes) | 14a |
| Adapter boot config | `mai-api/config/adapters.toml` | Development adapter defaults (Ollama), model alias map | 14b |
| E2E inference tests | `mai-api/tests/e2e_inference.sh` | Curl-based verification: chat, embed, SSE, aliases, errors | 14b |
| SDK integration tests | `mai-api/tests/sdk_integration.py` | Chat, streaming, embeddings, models, health, auth, rate limiting (7 categories) | 14c |
| Scheduler registry tests | `mai-scheduler/src/registry.rs` `#[cfg(test)]` | Register, duplicate, remove, find by model/GPU, request tracking, overload (11 tests) | 15 |
| Scheduler alias tests | `mai-scheduler/src/aliases.rs` `#[cfg(test)]` | Resolve known, passthrough unknown, has_alias, reload, list (6 tests) | 15 |
| Scheduler placement tests | `mai-scheduler/src/placement.rs` `#[cfg(test)]` | Least-loaded, VRAM tiebreaker, overload filter, continuation affinity, custom scorer (10 tests) | 15 |
| Scheduler default tests | `mai-scheduler/src/default.rs` `#[cfg(test)]` | Schedule, alias passthrough, backpressure, preferred backend, 100-thread concurrent (14 tests) | 15, 17 |
| Topology collector tests | `mai-scheduler/src/topology/collector.rs` `#[cfg(test)]` | nvidia-smi parsing, link types, CPU affinity, degenerate cases (11 tests) | 16 |
| Topology graph tests | `mai-scheduler/src/topology/graph.rs` `#[cfg(test)]` | Graph construction, edge costs, NVLink detection, NUMA, metrics update (8 tests) | 16 |
| Topology analysis tests | `mai-scheduler/src/topology/analysis.rs` `#[cfg(test)]` | Floyd-Warshall, best pairs/quads, NVLink cliques, CPU affinity groups (12 tests) | 16 |
| Topology refresh tests | `mai-scheduler/src/topology/refresh.rs` `#[cfg(test)]` | Anomaly detection, thermal throttle, VRAM exhaustion, metrics refresh (7 tests) | 16 |
| Topology mod tests | `mai-scheduler/src/topology/mod.rs` `#[cfg(test)]` | Default config, flat topology, single GPU penalty (3 tests) | 16 |
| Topology integration | `mai-scheduler/tests/topology_integration.rs` | Full pipeline: parse fixtures -> graph -> analysis -> penalty, config sensitivity (16 tests) | 16 |
| KV sequence tests | `mai-scheduler/src/kv/sequence.rs` `#[cfg(test)]` | Memory estimation, touch/request tracking, EMA gap, eviction/readmission lifecycle (11 tests) | 17 |
| KV eviction tests | `mai-scheduler/src/kv/eviction.rs` `#[cfg(test)]` | Multi-factor scoring, idle/size/priority/reuse components, system immunity (10 tests) | 17 |
| KV guard tests | `mai-scheduler/src/kv/guard.rs` `#[cfg(test)]` | Min residency, readmit penalty, rate limiting, eviction history (8 tests) | 17 |
| KV trigger tests | `mai-scheduler/src/kv/triggers.rs` `#[cfg(test)]` | Proactive/eviction/emergency thresholds, on-demand, boundary cases (8 tests) | 17 |
| KV manager tests | `mai-scheduler/src/kv/mod.rs` `#[cfg(test)]` | Allocate/deallocate, can_fit, eviction candidates, perform_eviction, emergency bypass (16 tests) | 17 |
| KV integration tests | `mai-scheduler/src/default.rs` `#[cfg(test)]` | KV attachment, cluster metrics with/without KV, release deallocates, can_fit budget (5 tests) | 17 |
| Batch metrics tests | `mai-scheduler/src/batch/metrics.rs` `#[cfg(test)]` | Empty snapshot, record steps, rolling window, admission/eviction rates, percentiles (9 tests) | 18 |
| Batch admission tests | `mai-scheduler/src/batch/admission.rs` `#[cfg(test)]` | Dual-threshold regions, boundary cases, priority/length checks, config update (14 tests) | 18 |
| Batch preemption tests | `mai-scheduler/src/batch/preemption.rs` `#[cfg(test)]` | Emergency threshold, victim selection, system immunity, scoring formula (10 tests) | 18 |
| Batch builder tests | `mai-scheduler/src/batch/builder.rs` `#[cfg(test)]` | Enqueue/admit, model mismatch, queue full, completion, batch limit, VRAM regions, preemption (14 tests) | 18 |
| Batch eviction scoring tests | `mai-scheduler/src/kv/eviction.rs` `#[cfg(test)]` | batch_member_protected, batch_aware_scoring_with_set (2 tests) | 18 |
| Batch integration tests | `mai-scheduler/src/default.rs` `#[cfg(test)]` | Builder created on register, removed on unregister, cluster metrics batch fields (3 tests) | 18 |
| Scoring unit tests | `mai-scheduler/src/scoring/*.rs` `#[cfg(test)]` | Multi-factor scorer and sub-score correctness (41 tests) | 19 |
| Session 19f schedule pipeline | `mai-scheduler/src/default.rs` `#[cfg(test)]` | 8 full DefaultScheduler.schedule() scenarios with topology, KV, batching, score breakdown, overload fallback, runtime rebuild | 19 |
| Metrics feedback tests | `mai-scheduler/src/metrics/*.rs` `#[cfg(test)]` | Lifecycle, feedback, health scoring, anomaly detection, ring buffer, MetricsCollector | 20 |
| Telemetry API tests | `mai-api/src/server.rs` and handler compile coverage | Startup config handles and telemetry handler wiring | 20 |
| OTA update tests | `mai-core/src/models/update.rs` `#[cfg(test)]` | No-identity manifest check, differential shards, resumable range download, license/tier validation, seasonal tier limits | 25 |
| Model lifecycle tests | `mai-core/src/models/lifecycle.rs` `#[cfg(test)]` | Load-benchmark-unload round trip, installed listing affinity, export config, affinity order | 25 |
| Preload planning tests | `mai-core/src/models/preload.rs` `#[cfg(test)]` | Sentinel, preferred, and affinity preload ordering | 25 |
| Update handler tests | `mai-api/src/handlers/updates.rs` `#[cfg(test)]` | Background download status progression | 25 |

---

## Configuration Files Index

| File | Purpose | Session |
|---|---|---|
| configs/scout.toml | Scout tier defaults (1x GPU, Ollama, Phi-4-mini Sentinel) | 06 |
| configs/ranger.toml | Ranger tier defaults (2x GPU, vLLM tensor parallel) | 06 |
| configs/pack-leader.toml | Pack Leader tier defaults (4+ GPU, full adapter fleet) | 06 |
| config/adapters.toml | Adapter boot config (Ollama defaults, model aliases) | 14b |
| config/scheduler.toml | Scheduler config (strategy, thresholds, model aliases) | 15 |
| config/topology.toml | Topology config (link weights, refresh interval, anomaly thresholds) | 16 |
| config/auth_keys.toml | API key auth config template (key hashes, rate limits) | 14c |
| config/kv.toml | KV cache config (budget, eviction weights, anti-thrash, triggers, model factors) | 17 |
| config/scoring.toml | Multi-factor scorer weights and normalization settings; autoloaded by API startup | 19 |
| config/metrics.toml | Feedback loop and metrics collector settings | 20 |
| tools/simulator/config.toml | Simulator hardware, workload, policy, and experiment settings | 21 |

---

*Island Mountain AI | Confidential*
