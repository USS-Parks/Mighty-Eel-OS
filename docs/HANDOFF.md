# MAI Founding Engineer Handoff

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Source:** MAI-BUILD-PROMPT-ROSTER-v2.md (restructured 2026-05-18, expanded 18 to 35 sessions)
**Status:** Phase A+B+C+D complete. Sessions 15-18 (Scheduler Core + GPU Topology + KV Cache Manager + Continuous Batching Engine) complete. Next: Session 19 (Multi-Factor Scorer).
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
| [MAI-BUILD-PROMPT-ROSTER-v2.md](MAI-BUILD-PROMPT-ROSTER-v2.md) | All 35 session prompts, deliverables, acceptance criteria (v1 archived) |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Trust model, component catalog, data flows |
| [CONVENTIONS.md](CONVENTIONS.md) | Code quality gates, monorepo layout, testing rules |
| [SESSION-LOG.md](SESSION-LOG.md) | Active progress tracker (Sessions 11-18) |
| [SESSION-LOG-ARCHIVE-01.md](SESSION-LOG-ARCHIVE-01.md) | Completed sessions (01-10) with full notes |
| [SESSION-RULES.md](SESSION-RULES.md) | Dependency enforcement, acceptance criteria, quality gates |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Deferred work, open questions |
| [INDEX.md](INDEX.md) | Master file index |

### Codebase (Phase C Complete)

5 Rust crates and 7 Python adapters are implemented. The mai-api crate (Session 11, all 5 sub-sessions complete) contains 27 source files (~12,400 lines), 3 integration test suites (16 tests), and proto/mai.proto (534 lines). The REST API has 20 endpoints plus a WebSocket at /v1/ws across 5 route groups (inference, models, health, system, streaming) with profile-based auth on all routes. The gRPC server runs on port 8421 with 6 MAI services + grpc.health.v1, tonic-reflection, and shared AppState. The mai-core kernel, mai-hil drivers, and mai-adapters framework are all production code with 86+ unit tests, 14 E2E integration tests, and 8 benchmarks passing. Session 11 adds 94 unit tests + 16 integration tests. See SESSION-LOG.md for detailed deliverable lists.

**CI fixes applied 2026-05-17:** (1) pytest collection failures fixed (missing `adapters/__init__.py`, added `conftest.py`). (2) `AdapterBase.__init__` now accepts optional config dict; all 6 non-Ollama adapters updated to match. (3) Stale test assertions corrected (llamacpp context_size, tensorrt ports).

**CI fixes applied 2026-05-21:** (1) 25 `cargo fmt` formatting diffs resolved across 14 files in 10 crates (accumulated drift from sandbox sessions without `cargo fmt`). (2) 13 type mismatches in test callsites fixed: `record_results()` now takes `&[ToolResult]` (tests passed owned `Vec`), `from_parsed()` now takes `&ParsedTopology` (tests passed owned), `inject_tool_result()` takes `&str` (test passed `String`). 21 files modified across 3 commits.

**Adapter contract alignment (2026-05-19):** Fixed 28 test failures across all 7 Python adapter test suites. Root causes: generate() dual-mode contract (stream vs non-stream), asyncio.to_thread incompatible with AsyncMock (replaced with `maybe_await()`), AdapterCapabilities field name mismatches, HealthStatus constructor drift, vllm embed API mismatch. All fixes in production code (base.py + 6 adapters), zero test modifications. 66/66 tests green. Sglang `self._raw_config` fix included.

**Response Cache (Session 10d, 2026-05-17):** Standalone `mai-core/src/cache.rs` module (627 lines, 12 unit tests). LRU eviction with TTL, memory budget enforcement, profile isolation, blake3 key hashing. Not yet integrated into scheduler or hotswap (deferred to Session 12+ when vault provides proper entry points). Types added to `mai-core/src/types.rs`.

**Vault Integration (Session 12, 2026-05-18):** New `mai-vault` crate (8 source files, ~3000 lines) implementing L2 vault layer. `mai-core/vault.rs` expanded from 49 to 788 lines with 7 traits: VaultInterface (original, unchanged), ModelStorage (ZFS ops), PqcProvider (ML-KEM-1024 + ML-DSA-87), TpmProvider (PCR-bound key sealing), ProfileStore (family profiles), AuditStore (hash-chained audit trail), VectorStore (Qdrant embeddings). FullVault super-trait with blanket impl. All implementations are structurally complete with correct NIST FIPS 203/204 key sizes, hash chain verification, cosine/euclidean/dot-product similarity, and 50+ unit tests. PQC library and ZFS linking deferred to local build.

**Agent/RAG Interface (Session 13, 2026-05-18):** New `mai-agent` crate (8 source files + 3 integration test files, ~5434 lines total). Context management with 4 truncation strategies (OldestFirst, MiddleOut, RelevanceScored, HardCutoff). Tool registry with OpenAI-compatible function format, multi-step chain tracking, role-based access control. RAG pipeline with batch embedding, cosine similarity semantic cache, profile-isolated retrieval. STT manager with PCM silence detection, audio buffering, Whisper large-v3 default. Agentic task manager with per-profile concurrency limits, resource budgets (tokens, tool calls, duration), submit/poll/cancel lifecycle. 61 unit tests + 16 integration tests. All types reference real mai-core exports.

**Session 14b complete (2026-05-19):** Real inference path wired end-to-end. HTTP requests now produce real tokens from real adapters. AdapterManager starts at boot from config/adapters.toml, registers with Scheduler, and shuts down cleanly. All 4 inference handlers (chat, embeddings, structured, function_call) call real adapter methods. SSE streaming reads from IPC event channel via new generate_stream_channel() method. Model alias resolution maps user-facing names to adapter+model pairs. Zero placeholder content remains. AdapterCrashed (MAI-3005) error variant added. All 3 integration test suites updated. e2e_inference.sh verification script created.

**Session 14c complete (2026-05-20):** API/SDK route alignment, auth hardening, SDK streaming. Added /v1/completions and /v1/power/state SDK-compat routes. Replaced header-trust auth (X-IM-Profile) with API key validation (X-IM-Auth-Token) using SHA-256 hashing, per-key sliding window rate limiting (default 60/min, MAI-4005 429 response). First-boot admin key generation (printed to stdout, never logged). Python SDK: real SSE streaming (sync Iterator + async AsyncIterator), retry with exponential backoff, health_check() convenience. SDK integration test suite with 7 test categories. No NotImplementedError stubs remain. New dependencies: sha2, hex, uuid. Config template: config/auth_keys.toml. Build docs: docs/BUILD.md. KNOWN-ISSUES.md: Issues #3, #4, #5 marked resolved. CI green: all 4 gates passing.

**Scheduler Core Architecture (Session 15, 2026-05-20):** New `mai-scheduler` crate (7 source files, ~1886 lines, 41+ unit tests). Object-safe `Scheduler` trait with `&self` methods for Arc<dyn Scheduler> compatibility in axum State. `DefaultScheduler` composes `InstanceRegistry` (DashMap-backed, lock-free), `PlacementEngine` (pluggable ScoringFn, least-loaded + continuation affinity for KV cache locality), and `AliasResolver` (user-facing names to backend models with preferred backends). Backpressure: System priority bypasses queue limits; Normal/Background rejected when overloaded. Atomic counters for total_routed/total_rejected. 100-thread concurrent scheduling test passes. Integrated into all 4 REST inference handlers, gRPC inference handler, and SSE streaming. Legacy mai-core Scheduler retained for HotSwapManager (migration deferred to Session 22). Config loaded from config/scheduler.toml with 5 model aliases.

**GPU Topology (Session 16, 2026-05-20):** Topology discovery module added to mai-scheduler (5 source files, ~2018 lines, 41 unit tests + 18 integration tests). Parses nvidia-smi topo -m output into weighted GPU interconnect graph with NVLink/PCIe/CPU-bridge/cross-socket edge costs. Precomputes best GPU pairs/quads, NVLink cliques (Bron-Kerbosch), Floyd-Warshall path cost matrix, CPU affinity groups. PlacementEngine gains topology_penalty() method for hardware-aware scoring. Configurable link weights via config/topology.toml. Periodic metrics refresh with anomaly detection (thermal throttle, VRAM exhaustion, stuck utilization). topology_penalty NOT wired into default scorer yet (Session 19 integrates multi-factor scoring). Fixture files for 1/2/4/8-GPU topologies with full integration test suite.

**KV Cache Manager (Session 17, 2026-05-20):** KV cache management subsystem added to mai-scheduler (6 source files in kv/ module, ~2292 lines, 53 unit tests + 5 integration tests). KvCacheManager trait (object-safe, Send+Sync) with HeuristicKvCacheManager concrete implementation. DashMap for lock-free sequence reads, AtomicU64 for used_bytes, Mutex<ThrashGuard> only for sequential eviction decisions. Multi-factor eviction scoring: idle time + size + priority penalty - reuse prediction. System priority sequences immune (score -1000). Anti-thrashing: minimum residency (30s), recently-evicted penalty (-100), rate limiter (10/sec). Three-tier triggers: proactive (75%, prepare candidates), standard (85%, evict with guards), emergency (95%, bypass residency). Scheduler integration: kv_manager field on DefaultScheduler, can_fit() advisory check in schedule(), deallocate on release_sequence(), ClusterMetrics gains kv_active_sequences/kv_used_bytes/kv_total_bytes. Config via config/kv.toml with 5 model memory factor entries. batch_contribution placeholder for Session 18.

**Continuous Batching Engine (Session 18, 2026-05-20):** New batch/ module in mai-scheduler (5 source files, ~1915 lines, 52 tests). BatchBuilder per-instance orchestrator with 4-phase build_step() cycle: remove completed, emergency preemption, admission drain, record metrics. Dual-threshold VRAM admission control (aggressive <80%, selective 80-90%, eviction-required >90%). Emergency preemption at 95% VRAM targeting sequences closest to completion or lowest priority. System priority never preempted. BatchMetrics with rolling-window averages, admission rate, wait time P50/P95/P99. Integrated into DefaultScheduler: DashMap<InstanceId, Mutex<BatchBuilder>> created on register_instance(), cluster_metrics() aggregates batch stats. KV eviction batch_contribution wired: active batch members get -100 eviction score protection (was 0.0 placeholder). All configurations TOML-deserializable with serde defaults.

**Immediate next step:** Execute **Session 19** (Multi-Factor Scorer). Sessions 15-18 (Scheduler Core + GPU Topology + KV Cache Manager + Continuous Batching) are complete. The scorer integrates topology_penalty, KV eviction cost, batch utilization, and queue depth into a single composite scoring function for the PlacementEngine. The scheduler track (15-21, 32-33) is the critical path. Security track (26-28) and application track (29-31) can now run in parallel.

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

**14a -> 14b -> 14c -> 15 -> 16 -> 17 -> 18 -> 19 -> 20 -> 21 -> 32 -> 33 -> 34 -> 35** (14 sessions sequential, 15-17 done)

Parallel tracks (after 14c completes):
- Track A: Scheduler (15-21, 32-33) - critical path
- Track B: Security (26-28) - independent after 14c
- Track C: Applications (29-31) - independent after 14c
- Track D: Power/Lifecycle (22-25) - depends on Session 19

Realistic remaining calendar: 35-49 Cowork sessions, 24-35 calendar days. See MAI-BUILD-PROMPT-ROSTER-v2.md for effort estimates.

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
