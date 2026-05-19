# MAI Founding Engineer Handoff

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Source:** MAI-BUILD-PROMPT-ROSTER.md (Session 65, 2026-05-15)
**Status:** Phase A+B+C(partial) complete. Session 12 complete. Next: Session 13 (Agent/RAG Interface).
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
| [MAI-BUILD-PROMPT-ROSTER.md](MAI-BUILD-PROMPT-ROSTER.md) | All 18 session prompts, deliverables, acceptance criteria |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Trust model, component catalog, data flows |
| [CONVENTIONS.md](CONVENTIONS.md) | Code quality gates, monorepo layout, testing rules |
| [SESSION-LOG.md](SESSION-LOG.md) | Active progress tracker (Sessions 11-18) |
| [SESSION-LOG-ARCHIVE-01.md](SESSION-LOG-ARCHIVE-01.md) | Completed sessions (01-10) with full notes |
| [SESSION-RULES.md](SESSION-RULES.md) | Dependency enforcement, acceptance criteria, quality gates |
| [KNOWN-ISSUES.md](KNOWN-ISSUES.md) | Deferred work, open questions |
| [INDEX.md](INDEX.md) | Master file index |

### Codebase (Phase B Complete)

5 Rust crates and 7 Python adapters are implemented. The mai-api crate (Session 11, all 5 sub-sessions complete) contains 27 source files (~12,400 lines), 3 integration test suites (16 tests), and proto/mai.proto (534 lines). The REST API has 20 endpoints plus a WebSocket at /v1/ws across 5 route groups (inference, models, health, system, streaming) with profile-based auth on all routes. The gRPC server runs on port 8421 with 6 MAI services + grpc.health.v1, tonic-reflection, and shared AppState. The mai-core kernel, mai-hil drivers, and mai-adapters framework are all production code with 86+ unit tests, 14 E2E integration tests, and 8 benchmarks passing. Session 11 adds 94 unit tests + 16 integration tests. See SESSION-LOG.md for detailed deliverable lists.

**CI fixes applied 2026-05-17:** (1) pytest collection failures fixed (missing `adapters/__init__.py`, added `conftest.py`). (2) `AdapterBase.__init__` now accepts optional config dict; all 6 non-Ollama adapters updated to match. (3) Stale test assertions corrected (llamacpp context_size, tensorrt ports). **Still needed:** run `cargo fmt` locally (Rust formatting drift), fix Sglang's `self._raw_config` reference (should be `self._config`).

**Response Cache (Session 10d, 2026-05-17):** Standalone `mai-core/src/cache.rs` module (627 lines, 12 unit tests). LRU eviction with TTL, memory budget enforcement, profile isolation, blake3 key hashing. Not yet integrated into scheduler or hotswap (deferred to Session 12+ when vault provides proper entry points). Types added to `mai-core/src/types.rs`.

**Vault Integration (Session 12, 2026-05-18):** New `mai-vault` crate (8 source files, ~3000 lines) implementing L2 vault layer. `mai-core/vault.rs` expanded from 49 to 788 lines with 7 traits: VaultInterface (original, unchanged), ModelStorage (ZFS ops), PqcProvider (ML-KEM-1024 + ML-DSA-87), TpmProvider (PCR-bound key sealing), ProfileStore (family profiles), AuditStore (hash-chained audit trail), VectorStore (Qdrant embeddings). FullVault super-trait with blanket impl. All implementations are structurally complete with correct NIST FIPS 203/204 key sizes, hash chain verification, cosine/euclidean/dot-product similarity, and 50+ unit tests. PQC library and ZFS linking deferred to local build.

**Immediate next step:** Execute **Session 13** (Agent/RAG Interface: tool calling execution, audio/STT binary frame processing, RAG pipeline integration with Qdrant). Session 13 depends on Sessions 08 and 12 (both complete).

---

## Five Things That Will Bite You If You Ignore Them

**1. The HIL is the moat.** The hardware interface layer survives hardware generations. The TetraMem MX100 arrives in 2028. The HIL is what makes that transition painless. Cut corners here and you become a one-generation company.

**2. Adapters are disposable. The core is not.** Backend adapters (Sessions 08-09) can be rewritten when Ollama's API changes or vLLM ships v2. The core kernel (Session 07) and API surface (Session 11) cannot be rewritten without breaking every application above them.

**3. Air-gap is not a checkbox.** It is an architectural constraint affecting every session. Every component must work with zero network access. If you find yourself writing `if air_gap_mode:` conditionals, you have already failed. The default is air-gapped. Network is the exception.

**4. PQC is ahead of schedule, on purpose.** ML-KEM and ML-DSA deployment in 2026 puts Island Mountain ahead of the NIST 2030 deadline by four years. This is a competitive advantage. Session 12 builds it.

**5. The quantum memristor transition is not science fiction.** TetraMem has shipping eval hardware. The HIL and adapter framework are designed so a TetraMem adapter slots in without changing a single line of core kernel or application code. If your implementation violates that property, you have failed the most important test.

---

## Critical Path (Remaining)

The longest remaining dependency chain:

**12 -> 15 -> 17 -> 18** (4 sessions sequential, Session 11 now complete)

Sessions that can overlap (if multiple sessions available):
- Sessions 14, 15, 16 can run in parallel (different subsystems, shared dependencies met after 12)

Realistic remaining calendar: 12-16 Cowork/Code sessions (Session 11 now complete).

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

Session 18 concludes with this checklist. Every item must pass before the MAI ships on any hardware:

- [ ] All Session 17 tests pass
- [ ] 72-hour burn-in passes on representative Scout hardware
- [ ] 72-hour burn-in passes on representative Ranger hardware
- [ ] Air-gap verification passes 72-hour endurance
- [ ] PQC encryption verified on all vault data
- [ ] Audit trail integrity verified over 72-hour period
- [ ] First-boot completes in <3 minutes
- [ ] Model update via USB verified
- [ ] All 7 adapters health-check pass
- [ ] Power state transitions all verified on hardware
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
- [MAI-BUILD-PROMPT-ROSTER.md](MAI-BUILD-PROMPT-ROSTER.md): Complete session prompts and deliverables

---

*Document derived from MAI-BUILD-PROMPT-ROSTER.md | 2026-05-15 | Island Mountain AI | Confidential*
