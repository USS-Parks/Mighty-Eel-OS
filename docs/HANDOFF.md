# MAI Founding Engineer Handoff

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Source:** MAI-BUILD-PROMPT-ROSTER-v2.md (restructured 2026-05-18, expanded to 46 sessions with Lamprey compliance governance)
**Status:** Sessions 1-25 are complete. Session 25 landed on 2026-05-22 with OTA manifest/differential update primitives, tier/license validation, lifecycle load/unload/benchmark/export operations, affinity preload planning, REST benchmark/update routes, and the update protocol spec.
**Archive:** Detailed Phase A+B code inventory and onboarding walkthrough archived to [HANDOFF-ARCHIVE-01.md](HANDOFF-ARCHIVE-01.md) on 2026-05-17.

---

## What You Are Picking Up

The MAI is the core inference abstraction layer for IM-OS, Island Mountain's sovereign data and identity operating system. It sits between all application logic (chat, document AI, medical records, home automation, legacy preservation) and all inference hardware (NVIDIA GPUs today, AMD GPUs, classical memristors in 2028, quantum memristors in 2030+).

The inference engine is a plugin. The data sovereignty layer is the product.

---

## Current State

### Governance Documents

| Document | Purpose |
|---|---|
| [MAI-BUILD-PROMPT-ROSTER-v2.md](MAI-BUILD-PROMPT-ROSTER-v2.md) | All 46 session prompts, deliverables, acceptance criteria (v1 archived) |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Trust model, component catalog, data flows |
| [CONVENTIONS.md](CONVENTIONS.md) | Code quality gates, monorepo layout, testing rules |
| [SESSION-LOG.md](SESSION-LOG.md) | Active progress tracker (current baseline: Sessions 1-25 complete) |
| [SESSION-LOG-ARCHIVE-01.md](SESSION-LOG-ARCHIVE-01.md) | Completed sessions (01-10) with full notes |
| [SESSION-RULES.md](SESSION-RULES.md) | Dependency enforcement, acceptance criteria, quality gates |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Deferred work, open questions |
| [INDEX.md](INDEX.md) | Master file index |

### Codebase (Current Baseline)

The workspace now contains the core MAI crates plus `mai-vault`, `mai-agent`, `mai-scheduler`, SDK crates, the package builder, and 7 Python backend adapters. The REST/gRPC API, vault interfaces, agent/RAG layer, scheduler foundation, topology graph, KV cache manager, continuous batching engine, scoring module, metrics feedback loop, simulator, power/sentinel path, model install/remove seams, and OTA/model lifecycle layer are all present.

**CI fixes applied 2026-05-17:** (1) pytest collection failures fixed (missing `adapters/__init__.py`, added `conftest.py`). (2) `AdapterBase.__init__` now accepts optional config dict; all 6 non-Ollama adapters updated to match. (3) Stale test assertions corrected (llamacpp context_size, tensorrt ports).

**CI fixes applied 2026-05-21:** (1) 25 `cargo fmt` formatting diffs resolved across 14 files in 10 crates (accumulated drift from sandbox sessions without `cargo fmt`). (2) 13 type mismatches in test callsites fixed: `record_results()` now takes `&[ToolResult]` (tests passed owned `Vec`), `from_parsed()` now takes `&ParsedTopology` (tests passed owned), `inject_tool_result()` takes `&str` (test passed `String`). 21 files modified across 3 commits.

**Adapter contract alignment (2026-05-19):** Fixed 28 test failures across all 7 Python adapter test suites. Root causes: generate() dual-mode contract (stream vs non-stream), asyncio.to_thread incompatible with AsyncMock (replaced with `maybe_await()`), AdapterCapabilities field name mismatches, HealthStatus constructor drift, vllm embed API mismatch. All fixes in production code (base.py + 6 adapters), zero test modifications. 66/66 tests green. Sglang `self._raw_config` fix included.

**Response Cache (Session 10d, 2026-05-17):** Standalone `mai-core/src/cache.rs` module (627 lines, 12 unit tests). LRU eviction with TTL, memory budget enforcement, profile isolation, blake3 key hashing. Not yet integrated into scheduler or hotswap (deferred to Session 12+ when vault provides proper entry points). Types added to `mai-core/src/types.rs`.

**Vault Integration (Session 12, 2026-05-18):** New `mai-vault` crate (8 source files, ~3000 lines) implementing L2 vault layer. `mai-core/vault.rs` expanded from 49 to 788 lines with 7 traits: VaultInterface (original, unchanged), ModelStorage (ZFS ops), PqcProvider (ML-KEM-1024 + ML-DSA-87), TpmProvider (PCR-bound key sealing), ProfileStore (family profiles), AuditStore (hash-chained audit trail), VectorStore (Qdrant embeddings). FullVault super-trait with blanket impl. All implementations are structurally complete with correct NIST FIPS 203/204 key sizes, hash chain verification, cosine/euclidean/dot-product similarity, and 50+ unit tests. PQC library and ZFS linking deferred to local build.

**Agent/RAG Interface (Session 13, 2026-05-18):** New `mai-agent` crate (8 source files + 3 integration test files, ~5434 lines total). Context management with 4 truncation strategies (OldestFirst, MiddleOut, RelevanceScored, HardCutoff). Tool registry with OpenAI-compatible function format, multi-step chain tracking, role-based access control. RAG pipeline with batch embedding, cosine similarity semantic cache, profile-isolated retrieval. STT manager with PCM silence detection, audio buffering, Whisper large-v3 default. Agentic task manager with per-profile concurrency limits, resource budgets (tokens, tool calls, duration), submit/poll/cancel lifecycle. 61 unit tests + 16 integration tests. All types reference real mai-core exports.

**Session 14b complete (2026-05-19):** Real inference path wired end-to-end. HTTP requests now produce real tokens from real adapters. AdapterManager starts at boot from config/adapters.toml, registers with Scheduler, and shuts down cleanly. All 4 inference handlers (chat, embeddings, structured, function_call) call real adapter methods. SSE streaming reads from IPC event channel via new generate_stream_channel() method. Model alias resolution maps user-facing names to adapter+model pairs. Zero placeholder content remains. AdapterCrashed (MAI-3005) error variant added. All 3 integration test suites updated. e2e_inference.sh verification script created.

**Session 14c complete (2026-05-20):** API/SDK route alignment, auth hardening, SDK streaming. Added /v1/completions and /v1/power/state SDK-compat routes. Replaced header-trust auth (X-IM-Profile) with API key validation (X-IM-Auth-Token) using SHA-256 hashing, per-key sliding window rate limiting (default 60/min, MAI-4005 429 response). First-boot admin key generation (printed to stdout, never logged). Python SDK: real SSE streaming (sync Iterator + async AsyncIterator), retry with exponential backoff, health_check() convenience. SDK integration test suite with 7 test categories. No NotImplementedError stubs remain. New dependencies: sha2, hex, uuid. Config template: config/auth_keys.toml. Build docs: docs/BUILD.md. KNOWN-ISSUES.md: Issues #3, #4, #5 marked resolved. CI green: all 4 gates passing.

**Scheduler Core Architecture (Session 15, 2026-05-20):** New `mai-scheduler` crate (7 source files, ~1886 lines, 41+ unit tests). Object-safe `Scheduler` trait with `&self` methods for Arc<dyn Scheduler> compatibility in axum State. `DefaultScheduler` composes `InstanceRegistry` (DashMap-backed, lock-free), `PlacementEngine` (pluggable ScoringFn, least-loaded + continuation affinity for KV cache locality), and `AliasResolver` (user-facing names to backend models with preferred backends). Backpressure: System priority bypasses queue limits; Normal/Background rejected when overloaded. Atomic counters for total_routed/total_rejected. 100-thread concurrent scheduling test passes. Integrated into all 4 REST inference handlers, gRPC inference handler, and SSE streaming. Legacy mai-core Scheduler retained for HotSwapManager (migration deferred to Session 22). Config loaded from config/scheduler.toml with 5 model aliases.

**GPU Topology (Session 16, 2026-05-20):** Topology discovery module added to mai-scheduler (5 source files, ~2018 lines, 41 unit tests + 18 integration tests). Parses nvidia-smi topo -m output into weighted GPU interconnect graph with NVLink/PCIe/CPU-bridge/cross-socket edge costs. Precomputes best GPU pairs/quads, NVLink cliques (Bron-Kerbosch), Floyd-Warshall path cost matrix, CPU affinity groups. PlacementEngine gains topology_penalty() method for hardware-aware scoring. Configurable link weights via config/topology.toml. Periodic metrics refresh with anomaly detection (thermal throttle, VRAM exhaustion, stuck utilization). Topology is now wired into the Session 19 multi-factor scorer and API startup path. Fixture files for 1/2/4/8-GPU topologies with full integration test suite.

**KV Cache Manager (Session 17, 2026-05-20):** KV cache management subsystem added to mai-scheduler (6 source files in kv/ module, ~2292 lines, 53 unit tests + 5 integration tests). KvCacheManager trait (object-safe, Send+Sync) with HeuristicKvCacheManager concrete implementation. DashMap for lock-free sequence reads, AtomicU64 for used_bytes, Mutex<ThrashGuard> only for sequential eviction decisions. Multi-factor eviction scoring: idle time + size + priority penalty - reuse prediction. System priority sequences immune (score -1000). Anti-thrashing: minimum residency (30s), recently-evicted penalty (-100), rate limiter (10/sec). Three-tier triggers: proactive (75%, prepare candidates), standard (85%, evict with guards), emergency (95%, bypass residency). Scheduler integration: kv_manager field on DefaultScheduler, can_fit() advisory check in schedule(), deallocate on release_sequence(), ClusterMetrics gains kv_active_sequences/kv_used_bytes/kv_total_bytes. Config via config/kv.toml with 5 model memory factor entries. batch_contribution placeholder for Session 18.

**Continuous Batching Engine (Session 18, 2026-05-20):** New batch/ module in mai-scheduler (5 source files, ~1915 lines, 52 tests). BatchBuilder per-instance orchestrator with 4-phase build_step() cycle: remove completed, emergency preemption, admission drain, record metrics. Dual-threshold VRAM admission control (aggressive <80%, selective 80-90%, eviction-required >90%). Emergency preemption at 95% VRAM targeting sequences closest to completion or lowest priority. System priority never preempted. BatchMetrics with rolling-window averages, admission rate, wait time P50/P95/P99. Integrated into DefaultScheduler: DashMap<InstanceId, Mutex<BatchBuilder>> created on register_instance(), cluster_metrics() aggregates batch stats. KV eviction batch_contribution wired: active batch members get -100 eviction score protection (was 0.0 placeholder). All configurations TOML-deserializable with serde defaults.

**Multi-Factor Scorer (Session 19, complete 2026-05-22):** New `scoring/` module in mai-scheduler plus API startup integration. `MultiFactorScorer` orchestrator combines 5 sub-scorers: latency penalty, memory pressure, topology penalty, eviction cost, and batching benefit. Continuation routing: warm KV cache hit gets an absolute bonus (default 10.0). API startup loads scheduler, topology, KV, and `config/scoring.toml` before publishing `Arc<dyn Scheduler>`. Placement decisions now include score breakdown diagnostics (`multi-factor(lat=... mem=... topo=... evict=... batch=... total=...)`). Session 19f coverage added: 8 full `DefaultScheduler.schedule()` scenarios verify topology, KV, batching, latency, memory, overload fallback, runtime rebuild, and score breakdown behavior.

**Feedback Loop + Metrics (Session 20, complete):** `mai-scheduler/src/metrics/` adds lifecycle tracking, completion feedback, health scoring, anomaly detection, and ring-buffer storage. `MetricsCollector` is stored in AppState. `mai-api/src/handlers/telemetry.rs` exposes scheduler metrics, per-instance metrics, health, and anomaly endpoints. `config/metrics.toml` is present.

**Simulation Framework (Session 21, complete):** `tools/simulator/` contains the discrete-event simulation engine, GPU model, workload generator, KV policy implementations, metrics/reporting, experiment runner, config, and README for offline tuning of scoring/KV/batching policy.

**Power + Sentinel (Sessions 22-23, complete):** Power state machine refactor lives in `mai-core/src/power/` with scheduler-facing controller in `mai-scheduler/src/power.rs`. Sentinel mode and promotion path live in `mai-core/src/sentinel/` with estimator/runtime/promotion/warmup modules and `config/sentinel.toml`.

**OTA + Model Lifecycle (Session 25, complete 2026-05-22):** `mai-core/src/models/update.rs` provides mockable HTTPS update transport boundaries, manifest comparison, differential shard planning, resumable shard download, tier/license validation, and seasonal bundle limits. `lifecycle.rs` provides installed model listing, load/unload, deterministic benchmark, export TOML, and affinity tracking. `preload.rs` plans sentinel-first, preferred-model, then affinity-based background preload order. REST routes now include benchmark and update check/download/status endpoints. `docs/UPDATE-PROTOCOL.md` specifies the privacy-preserving third-party mirror contract.

**HIPAA Compliance Engine (Session 38, complete 2026-05-22 — first per-regulation Lamprey module):** New `mai-compliance` crate. `phi.rs` (~400 lines) ships a detector for all 18 HIPAA Safe Harbor identifiers with `PhiConfidence` tiers (Possible / Probable / Explicit, correctly ordered for `>=` gating) and a `PhiReport` aggregator with blake3-hashed matched-text (never raw). `baa.rs` (~340 lines) exposes `BaaEnforcer` over `BaaMode { Standard, Strict, Custom { max_cloud_confidence, never_leave_local } }` returning a `BaaDecision` over the report — pure, never sees raw text. `deid.rs` (~260 lines) is the zero-false-negative redactor: replaces PHI spans with `[PHI:<kind>]` placeholders and emits a composite re-identification `RiskScore`. `medical_entities.rs` (~260 lines) adds `IcdValidator`, `MedicationDictionary` (RxNorm-style baseline), and `parse_lab_values()` for routing enrichment. `config/compliance/hipaa.toml` documents all three BAA modes inline. p99 detection under 10ms verified by `tests/phi_perf.rs` on a 500-sample mixed corpus. 42 unit tests + 1 perf acceptance; workspace `fmt --check` and `clippy -- -D warnings` clean. The crate is intentionally standalone — wiring into the Session 37 rule-engine FactSet is deferred to Session 41 so the integration shape can be co-designed with Audit (42) and Reports (43).

**Lamprey Policy Framework (Session 37, complete 2026-05-22 — second Lamprey layer):** Programmable rule engine on top of the Session 36 router. `mai-router/src/rules/engine.rs` exposes `Rule { name, priority, condition, action, audit_level }` with a boolean condition tree (Match / All / Any / Not) and four actions (Allow / Deny / Reroute / Flag). `rules/modules.rs` defines `PolicyModule` and `PolicyModuleRegistry` with runtime install / enable-disable / load-from-TOML; thread-safe via RwLock. `pipeline.rs` composes classifier → entities → policy → budget → decision with per-stage microsecond `StageMetrics`; winning rule actions override default router precedence, Allow/Flag fall through. Three baseline modules ship: `rules-config/hipaa.toml` (PHI force-local + admin flag), `rules-config/itar.toml` (export-controlled deny), `rules-config/ocap.toml` (tribal force-local + sensitive-tribal deny). `tools/rule-tester/` is a CLI that loads a rules TOML + scenarios TOML and prints which rules fire with assertable `expect_action` / `expect_rule` per scenario — exit code = number of mismatches. 62 unit tests + 4 baseline-load tests + 3 rule-tester scenarios, all green; workspace `fmt --check` and `clippy -- -D warnings` clean.

**Lamprey Query Router (Session 36, complete 2026-05-22 — first Lamprey layer):** New `mai-router` crate. Five modules — `classifier` (regex-based five-level sensitivity), `entities` (medical/tribal/export-controlled dictionary with blake3-hashed matches), `cost` (per-role monthly budgets with soft + hard caps), `router` (the `Router` trait, `RoutingDecision` enum, and `DefaultRouter` composition), and `fallback` (cloud→local fallback chain with denied-short-circuit) — produce a deterministic decision with audit-grade reason for every request. Decision precedence: hard deny at Critical → export-controlled / tribal forced local → above cloud ceiling forced local → budget hard cap forced local → cloud default. `config/router.toml` ships baseline patterns, dictionaries, and budgets; entirely file-driven. p99 decision latency verified under 5ms by `tests/latency_budget.rs` on a 1,000-sample mixed corpus. 39 unit tests + 1 acceptance test, all green; workspace `fmt --check` + `clippy -- -D warnings` clean.

**Deployment Packaging (Session 35, complete 2026-05-22 — Gate C CLOSED):** Cross-platform launch + health-check + burn-in scripts in `scripts/` (bash + PowerShell pairs); `tools/smoke/smoke_client.py` is a stdlib-only end-to-end probe that exercises the public REST surface and serves as the Gate C "SDK runs against packaged deployment" evidence until proper L4-L5 scaffolds land in Sessions 29-31; `docs/DEPLOYMENT.md` is the operator guide covering quick start, configuration, health verification, burn-in workflow, and troubleshooting; `docs/KNOWN-ISSUES.md` is current. **The Core Platform Release is now shippable.** Hardware-dependent Phase 1 exit criteria (Scout/Ranger boot timings, two-GPU configs, 72h stability) emit a `phase1-deferred.txt` artifact in every burn-in run so the deferral is never silent.

**Integration Test Suite + System Validation (Session 34, complete 2026-05-22 — Gate C criteria met):** Audited existing integration coverage (~4,350 LOC across 14 test files spanning mai-api, mai-core, mai-scheduler, mai-adapters, mai-agent, mai-hil) and closed the four gaps: `mai-api/tests/system_integration.rs` adds 7 named tests for air-gap enforcement (3 cases against a mock SwitchReader), HTTP-level power state transitions (full Off→Sentinel→Full→Off cycle via the API), family-profiles permission matrix (Admin passes, Adult/Child/Guest denied on admin-only endpoints), and zero-data-leak (structural assertion that `AuditEntry` exposes no field that could carry prompts or responses). `docs/INTEGRATION-COVERAGE.md` maps all 16 coverage areas to test files with ✓/◐/✗ status and explicitly defers hardware-dependent Phase 1 exit criteria (Scout/Ranger boot timings, two-GPU configs, 72h stability) to Session 35 burn-in. `cargo fmt --check` clean; `cargo clippy --workspace -- -D warnings -A clippy::pedantic` clean; full workspace tests green.

**Multi-Instance Cross-GPU Scheduling + Soft Eviction (Session 33, complete 2026-05-22 — Gate C criteria met):** Five new primitives landed in `mai-scheduler/`: `kv/offload.rs` (soft eviction with explicit Active→Offloading→Offloaded→Restoring state machine and CPU pinned-memory budget), `kv/tiered.rs` (stateless hot/warm/cold tier controller proposing demote/promote/evict actions), `preemption.rs` (System>High>Normal>Background hierarchy + starvation-prevention priority boost on resume), `balancer.rs` (cross-instance migration scoring net benefit = load gap minus topology cost, sorted descending), `decision_cache.rs` (TTL-bounded (model_alias, priority, load_bucket) → ScheduleDecision cache with hit/miss counters). The primitives are intentionally standalone — they expose clean surfaces the next session (34, integration tests) and the policy runtime (41, Lamprey) will wire deeper. 31 new unit tests + 8 Gate C acceptance integration tests. 324 scheduler-lib tests total, full workspace green.

**Auth Hardening (Session 26, complete 2026-05-22 — Gate A criteria met):** The Session 14c auth surface (`mai-api/src/auth.rs`, 923 lines) was already substantial: SHA3-256 hashed API key store, sliding-window rate limiter, profile-permission matrix, first-boot admin key generation with one-time print. Session 26 closed three concrete gaps: (1) `generate_api_key()` now uses `rand::rngs::OsRng` instead of SHA3-of-time+pid+uuid; (2) `mai-sdk-rs::MaiClientConfig` gained an `api_key: Option<String>` field and an `auth_headers()` helper so the Session 11 finish-out has a place to apply auth; (3) `mai-api/tests/auth_gate_a.rs` adds 6 explicit acceptance tests against a strict AuthState (missing/invalid/valid/rate-limit/spoofing/exempt-health). `docs/SECURITY.md` documents the trust floor. All workspace tests pass.

**Production Trace Integration + Replay (Session 32, complete 2026-05-22 — Gate C criteria met):** `mai-scheduler/src/traces/capture.rs` writes opt-in NDJSON traces with daily rotation; session ids are blake3-hashed at capture time so the raw trace is unlinkable. The module was named `traces` (not `tracing` per the spec letter) because `tracing` is a workspace logging crate used across mai-scheduler; the name swap is the only deviation. `tools/trace-tools/` adds anonymize/reconstruct/calibrate Python scripts. `tools/simulator/` adds `trace_generator.py` (replays an NDJSON trace as a WorkloadGenerator preserving inter-request gaps), `hybrid.py` (combines a trace baseline with a synthetic spike), `replay_compare.py` (trace-driven multi-policy comparison harness — deterministic at (trace, seed, policy)), and `report.py` (Markdown / JSON report renderer with headline findings — designed for acquisition documentation). Privacy is structural: capture and anonymize both project to a documented allowlist and tests assert no prompt/response text leaks. 18 new Session 32 tests across Python + 4 new Rust capture tests; full suite at 114 Python + 293 Rust scheduler tests passing.

**Immediate next step:** Session 39 (ITAR/EAR Compliance Engine) — extends `mai-compliance` with USML category detection, technical-data classification, and dual-use technology rules. Same crate, additive surface. Sessions 27-28 (security baseline) and 29-31 (SDK + app scaffolds) remain safe parallel candidates and should land before final acquisition prep (Session 45).

---

## Five Things That Will Bite You If You Ignore Them

**1. The HIL is the moat.** The hardware interface layer survives hardware generations. The TetraMem MX100 arrives in 2028. The HIL is what makes that transition painless. Cut corners here and you become a one-generation company.

**2. Adapters are disposable. The core is not.** Backend adapters (Sessions 08-09) can be rewritten when Ollama's API changes or vLLM ships v2. The core kernel (Session 07) and API surface (Session 11) cannot be rewritten without breaking every application above them.

**3. Air-gap is not a checkbox.** It is an architectural constraint affecting every session. Every component must work with zero network access. If you find yourself writing `if air_gap_mode:` conditionals, you have already failed. The default is air-gapped. Network is the exception.

**4. PQC is ahead of schedule, on purpose.** ML-KEM and ML-DSA deployment in 2026 puts Island Mountain ahead of the NIST 2030 deadline by four years. This is a competitive advantage. Session 12 builds it.

**5. The quantum memristor transition is not science fiction.** TetraMem has shipping eval hardware. The HIL and adapter framework are designed so a TetraMem adapter slots in without changing a single line of core kernel or application code. If your implementation violates that property, you have failed the most important test.

---

## Critical Path (Remaining)

The longest remaining dependency chain (restructured):

**32 -> 33 -> 34 -> 35 -> 36 -> 37 -> 38 -> 39 -> 40 -> 41 -> 42 -> 43 -> 44 -> 45 -> 46**

Parallel tracks (after 14c completes):
- Track A: Scheduler (15-21, 32-33) - critical path
- Track B: Security (26-28) - independent after 14c
- Track C: Applications (29-31) - independent after 14c
- Track D: Power/Lifecycle (22-25) - depends on Session 19

See MAI-BUILD-PROMPT-ROSTER-v2.md for current 46-session effort estimates and the Lamprey compliance governance layer sequence.

---

## What Is NOT In Scope

These items are explicitly excluded. See [KNOWN-ISSUES.md](KNOWN-ISSUES.md) for the full list.

- L6 UI (React dashboard, onboarding wizard)
- Remote support tunnel (network service, not MAI)
- Landfall Council (multi-user chat variant, deferred)
- Full L4 agent logic (RAG pipeline internals, tool implementations)
- Full L5 application logic (only scaffolds are built)
- TetraMem adapter implementation (stub interface only)
- Photonic adapter implementation (stub interface only)

---

## Production Readiness Checklist

Session 35 concludes with this checklist. Every item must pass before the MAI ships on any hardware:

- [ ] All Session 34 tests pass
- [ ] 72-hour burn-in passes on representative Scout hardware
- [ ] 72-hour burn-in passes on representative Ranger hardware
- [ ] Air-gap verification passes 72-hour endurance
- [ ] PQC encryption verified on all vault data
- [ ] Audit trail integrity verified over 72-hour period
- [ ] First-boot completes in <3 minutes
- [ ] Model update via USB verified
- [ ] All 7 adapters health-check pass
- [ ] Power state transitions all verified on hardware
- [ ] Scheduler topology correctly maps hardware
- [ ] Documentation reviewed and complete
- [ ] Performance baseline stored for future regression detection

---

## Related Documents

- [ARCHITECTURE.md](ARCHITECTURE.md): System architecture and trust model
- [CONVENTIONS.md](CONVENTIONS.md): Coding standards and naming rules
- [PROJECT.md](PROJECT.md): Scope, phases, timeline
- [SESSION-RULES.md](SESSION-RULES.md): Session governance and quality gates
- [SESSION-LOG.md](SESSION-LOG.md): Session progress tracking
- [KNOWN-ISSUES.md](KNOWN-ISSUES.md): Limitations and deferred items
- [INDEX.md](INDEX.md): Master file index
- [MAI-BUILD-PROMPT-ROSTER-v2.md](MAI-BUILD-PROMPT-ROSTER-v2.md): Complete session prompts and deliverables (restructured)
- [MAI-BUILD-PROMPT-ROSTER.md](MAI-BUILD-PROMPT-ROSTER.md): Original session prompts (v1, archived)

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*
