# MAI Build Index

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Last Updated:** 2026-05-20 (Session 18)

---

## Project Governance Documents

These documents govern the MAI build. Read them before writing code.

| File | Purpose | Read When |
|---|---|---|
| [MAI-BUILD-PROMPT-ROSTER-v2.md](MAI-BUILD-PROMPT-ROSTER-v2.md) | Complete session prompts, deliverables, and acceptance criteria for all 35 sessions (v2 restructured) | Starting any session |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Tock-to-MAI architecture map, trust boundaries, component catalog, data flows, power state machine | Understanding system structure |
| [CONVENTIONS.md](CONVENTIONS.md) | Language assignments, code quality gates, monorepo layout, testing rules, git conventions | Writing any code |
| [HANDOFF.md](HANDOFF.md) | Founding engineer orientation, critical warnings, current state, what will bite you | First day on the project |
| [INDEX.md](INDEX.md) | This file. Master index of all project documents | Finding anything |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Out-of-scope items, deferred work, architectural limitations, open questions | Wondering "should I build this?" |
| [PROJECT.md](PROJECT.md) | Scope, 5-phase plan, 18-session timeline, effort estimates, coverage matrix | Understanding scope and schedule |
| [SESSION-LOG.md](SESSION-LOG.md) | Active session progress (Sessions 11-18) | Before and after each session |
| [SESSION-LOG-ARCHIVE-01.md](SESSION-LOG-ARCHIVE-01.md) | Completed sessions 01-10 (Phase A+B) with full notes and deliverable lists | Reviewing past session details |
| [HANDOFF-ARCHIVE-01.md](HANDOFF-ARCHIVE-01.md) | Archived onboarding walkthrough and Phase A+B code inventory | Reference only |
| [SESSION-RULES.md](SESSION-RULES.md) | Dependency enforcement, acceptance criteria protocol, quality gates, session workflow | Conducting any session |
| [IPC-PROTOCOL.md](IPC-PROTOCOL.md) | NDJSON IPC wire format spec for Rust-Python adapter communication | Working on adapter IPC (Sessions 14a-14c) |

---

## Session-to-Document Map

Each session produces specific deliverables. This table maps sessions to their primary output types.

### Phase A: Specification (Sessions 01-05)

| Session | Title | Primary Outputs |
|---|---|---|
| 01 | MAI Master Architecture Specification | Architecture doc (40-60 pages), project scaffold, dependency graph, glossary |
| 02 | Hardware Interface Layer (HIL) Specification | HIL trait defs (Rust), driver specs, power state machine diagram, air-gap daemon spec |
| 03 | Backend Adapter Framework Specification | Adapter traits (Rust + Python), per-backend specs (7 backends), lifecycle + sandboxing specs |
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

### Phase D: Scheduler Foundation (Sessions 15-18)

| Session | Title | Primary Outputs |
|---|---|---|
| 15 | Scheduler Core Architecture | mai-scheduler crate (7 files, 41+ tests), Scheduler trait, DefaultScheduler, API integration |
| 16 | GPU Topology Discovery + Weighted Graph | topology module (5 files, 41 unit tests), integration tests (16 tests), fixtures, config/topology.toml |
| 17 | KV Cache Manager | kv/ module (6 files, 53 unit tests + 5 integration tests), KvCacheManager trait, HeuristicKvCacheManager, config/kv.toml |
| 18 | Continuous Batching Engine | batch/ module (5 files, 52 tests), BatchBuilder, AdmissionController, PreemptionPolicy, BatchMetrics, eviction batch_contribution wired |

---

## Crate / Package Index (Post-Session 06)

After the project scaffold is created in Session 06, the monorepo will contain:

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
| adapters/ollama | Python | Untrusted | 06 (scaffold) | 08 |
| adapters/vllm | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/llamacpp | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tgi | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tensorrt | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/exllamav2 | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/sglang | Python | Untrusted | 06 (scaffold) | 09 |

---

## Test Suites Index (Updated Session 18)

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

---

## Configuration Files Index (Post-Session 17)

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

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*