# MAI Build Index

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Last Updated:** 2026-05-17

---

## Project Governance Documents

These documents govern the MAI build. Read them before writing code.

| File | Purpose | Read When |
|---|---|---|
| [MAI-BUILD-PROMPT-ROSTER.md](MAI-BUILD-PROMPT-ROSTER.md) | Complete session prompts, deliverables, and acceptance criteria for all 18 sessions | Starting any session |
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

### Phase D: System Code (Sessions 14-16)

| Session | Title | Primary Outputs |
|---|---|---|
| 14 | Sleep Mode + Power State Machine | Full power state machine, Sentinel mode, promotion path, auto-demotion, hardware integration |
| 15 | Model Management + OTA Update Pipeline | Model package format, USB air-gap install, network updates, lifecycle operations |
| 16 | L4-L5 Application Integration Scaffolds | 7 app scaffolds (Summit Chat, FamilyVault, Scribe, Legacy Engine, MedRecord, HomeBase, Estate AI) |

### Phase E: Testing + Packaging (Sessions 17-18)

| Session | Title | Primary Outputs |
|---|---|---|
| 17 | Integration Test Suite + System Validation | Phase 1 exit criteria tests, scenario tests, security validation, performance baseline |
| 18 | Deployment Packaging + Burn-In Protocol | .deb package, systemd services, Docker compose, first-boot automation, 72-hour burn-in, docs |

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
| adapters/ollama | Python | Untrusted | 06 (scaffold) | 08 |
| adapters/vllm | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/llamacpp | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tgi | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/tensorrt | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/exllamav2 | Python | Untrusted | 06 (scaffold) | 09 |
| adapters/sglang | Python | Untrusted | 06 (scaffold) | 09 |

---

## Configuration Files Index (Post-Session 06)

| File | Purpose | Session |
|---|---|---|
| configs/scout.toml | Scout tier defaults (1x GPU, Ollama, Phi-4-mini Sentinel) | 06 |
| configs/ranger.toml | Ranger tier defaults (2x GPU, vLLM tensor parallel) | 06 |
| configs/pack-leader.toml | Pack Leader tier defaults (4+ GPU, full adapter fleet) | 06 |

---

## Test Suites Index (Updated Session 12)

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
| Security tests | tests/integration/ | PQC integrity, tamper detection, sandbox enforcement | 17 |
| Scenario 