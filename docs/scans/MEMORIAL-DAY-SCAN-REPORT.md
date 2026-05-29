# Memorial Day Scan Report

**Project:** Island Mountain MAI / Lamprey OS
**Repository:** mai/ (monorepo — Cargo workspace + Python)
**Date:** 2026-05-25 (Memorial Day)
**Scanner:** `tools/local_gitdoctor_scan.py` — offline static audit, John-Dougherty-mirrored check families
**Commit:** 493de62 — "Semantically split adapter quality hotspots"
**Author:** Basho Parks, 2026-05-25 22:12:09 -0700

---

## Executive Summary

A comprehensive offline static audit was run against the full mai/ monorepo using the local GitDoctor-style scanner. The audit covers **58 checks** across 7 categories mirroring John Dougherty's GitDoctor review families: Security (16 checks), Performance (6), Code Quality (10), Configuration (7), Testing (6), Review Integrity (8), and Project Hygiene (5).

**Overall Score: 93/100 — 54 passed, 4 failed.**

All security, performance, configuration, project-hygiene, and testing checks pass at 100%. Four low-to-medium findings exist in Code Quality (3) and Review Integrity (1). No critical or high-severity items remain open across any category.

The full Cargo workspace compiles cleanly (cargo check --workspace — zero errors) and the working tree is clean (no uncommitted changes, no tracked debris).

---

## Repository Snapshot

| Metric | Count |
|---|---|
| Total tracked files (all types) | 105,431 |
| Rust files (.rs) | 285 |
| Python files (.py) | 244 |
| JavaScript/TypeScript files (.js/.ts) | 49 |
| **Code-bearing tracked files (.rs/.py/.js/.ts)** | **510** |
| Working tree status | Clean |
| May 2026 commits (since May 1) | 142 |
| Cargo workspace check | Pass (zero errors) |

---

## Category Scorecard

| Category | Score | Passed | Failed | Description |
|---|:---:|---:|---:|---|
| **Security** | 100/100 | 16 | 0 | All SEC-001..SEC-016 green |
| **Performance** | 100/100 | 6 | 0 | All PERF-001..PERF-006 green |
| **Configuration** | 100/100 | 7 | 0 | All CFG-001..CFG-007 green |
| **Project Hygiene** | 100/100 | 5 | 0 | All PRJ-001..PRJ-005 green |
| **Testing** | 100/100 | 6 | 0 | All TST-001..TST-006 green |
| **Code Quality** | 70/100 | 7 | 3 | QUA-001, QUA-008, QUA-009 |
| **Review Integrity** | 88/100 | 7 | 1 | REV-007 |
| **Overall** | **93/100** | **54** | **4** | |

---

## Failures — Detailed Findings

### QUA-001 — God files over 300 lines (MEDIUM)

**12 files exceed the 300-line threshold.** Large files may benefit from focused module extraction.

| File | Lines |
|---|---|
| mai-agent/src/tasks.rs | 869 |
| mai-agent/src/context.rs | 860 |
| mai-agent/src/tools.rs | 783 |
| mai-agent/src/rag.rs | 751 |
| mai-agent/src/types.rs | 749 |
| mai-adapters/src/manager.rs | 647 |
| mai-agent/src/stt.rs | 619 |
| mai-adapters/src/process.rs | 611 |
| compliance-dashboard/app.py | 456 |
| mai-adapters/src/bridge.rs | 373 |
| pps/openbao-trust-demo/main.py | 369 |
| mai-adapters/src/config.rs | 359 |

**Remediation:** mai-agent/ accounts for 6 of 12 files. Splitting context.rs, 	asks.rs, and 	ools.rs into focused submodules would yield the largest score improvement. The mai-adapters/ files were recently refactored from larger god files in the prior session; continued decomposition is warranted.

---

### QUA-008 — Modules with 15+ exports (LOW)

**8 files export 15 or more public items,** which can indicate unfocused module boundaries.

| File | Exports |
|---|---|
| mai-agent/src/types.rs | 39 |
| mai-adapters/src/bridge.rs | 27 |
| mai-agent/src/tasks.rs | 23 |
| mai-agent/src/tools.rs | 22 |
| mai-api/src/auth.rs | 22 |
| mai-agent/src/context.rs | 19 |
| mai-agent/src/rag.rs | 15 |
| mai-agent/src/stt.rs | 15 |

**Remediation:** These are concentrated in mai-agent/ (6 of 8). This is partially structural — 	ypes.rs serves as the crate's public API surface. Splitting 	ypes.rs into domain submodules (chat types, RAG types, tool types) would narrow the export surface.

---

### QUA-009 — Deeply nested code (MEDIUM)

**8 files contain code at 4+ indentation levels.** Deep nesting reduces readability and indicates complex control flow.

| File | Line | Nesting |
|---|---|---|
| dapters/ollama/client.py | 82 | ~4 levels |
| dapters/onnxruntime/adapter.py | 77 | ~4 levels |
| dapters/onnxruntime/client.py | 66 | ~4 levels |
| dapters/onnxruntime/client_helpers.py | 43 | ~4 levels |
| dapters/onnxruntime/config.py | 73 | ~4 levels |
| dapters/openai_compat/adapter.py | 86 | ~4 levels |
| dapters/openai_compat/adapter_helpers.py | 35 | ~4 levels |
| dapters/openai_compat/client.py | 175 | ~4 levels |

**Remediation:** All hits are in Python adapter code (Ollama, ONNX Runtime, OpenAI-compat). Extract inner loops and deeply-nested conditionals into helper functions. The QUA-009 adapter nesting commit (9b393c0) partially addressed this; remaining hits represent the residual complex control flow in adapter I/O dispatch.

---

### REV-007 — Duplicated boilerplate blocks (LOW)

**8 duplicate 6-line blocks detected across modules,** suggesting copy-forward implementation.

| First Location | Duplicate Location |
|---|---|
| mai-core/src/power/demotion.rs:12 | mai-core/src/power/mod.rs:92 |
| mai-api/src/types.rs:183 | mai-core/src/registry.rs:36 |
| mai-hil/src/drivers/amd.rs:125 | mai-hil/src/drivers/cpu.rs:130 |
| mai-hil/src/drivers/amd.rs:126 | mai-hil/src/drivers/cpu.rs:131 |
| mai-hil/src/drivers/amd.rs:127 | mai-hil/src/drivers/cpu.rs:132 |
| mai-hil/src/drivers/amd.rs:128 | mai-hil/src/drivers/cpu.rs:133 |
| mai-hil/src/drivers/amd.rs:147 | mai-hil/src/drivers/cpu.rs:149 |
| mai-hil/src/drivers/amd.rs:148 | mai-hil/src/drivers/cpu.rs:150 |

**Remediation:** The mai-hil/ driver blocks (6 of 8) are expected structural duplication between AMD and CPU driver stubs — both implement the same HIL trait contract. The mai-core/ and mai-api/ hits warrant a closer look to see if shared logic can be extracted into a common utility module.

---

## Passed Checks — Complete Summary

### Security (16/16 — 100%)

| Check | Description |
|---|---|
| SEC-001 | Dynamic code execution — not found |
| SEC-002 | XSS via HTML injection — not found |
| SEC-003 | SQL injection via string interpolation — not found |
| SEC-004 | Hardcoded API keys/secrets — not found |
| SEC-005 | Private keys in source — not found |
| SEC-006 | Hardcoded JWT tokens — not found |
| SEC-007 | Client-side secret exposure — not found |
| SEC-008 | CORS wildcard origin — not found |
| SEC-009 | Math.random() in security context — not found |
| SEC-010 | Insecure cookie configuration — not found |
| SEC-011 | Rate limiting — signal present |
| SEC-012 | Input validation — schema signals present |
| SEC-013 | Debug mode in config — not found |
| SEC-014 | Unprotected mutation routes — auth signals present |
| SEC-015 | File upload handling — validation signals present |
| SEC-016 | State-changing GET routes — not found |

### Performance (6/6 — 100%)

All PERF-001 through PERF-006 pass: no wait-in-.map(), no sync Node I/O blocking, no N+1 patterns, no JSON-in-loops, no sequential awaits needing parallelization, no unbounded growth in loops.

### Configuration (7/7 — 100%)

All CFG-001 through CFG-007 pass: no localhost URLs in non-test source, Docker images pinned, health endpoints present, .env.example found, TypeScript strict mode signals present, CI configuration found, Dockerfile present.

### Testing (6/6 — 100%)

All TST-001 through TST-006 pass: test files present, adequate test ratio, no empty test bodies, assertions present in tests, integration/e2e tests present, mock density within acceptable bounds.

### Project Hygiene (5/5 — 100%)

All PRJ-001 through PRJ-005 pass: no committed .env file, .gitignore complete, README present, lock files tracked, flat-structure ratio within bounds.

### Review Integrity (7/8 — 88%)

REV-001 through REV-006 and REV-008 pass: no documented APIs with placeholder bodies, adapter placeholder density within bounds, no polished claims with placeholders, error taxonomy adequately applied, silent error handlers within bounds, thin smoke assertions within bounds, comment-heavy implementations within bounds.

---

## Remediation Priority Matrix

| Priority | Check | Severity | Estimated Effort | Recommendation |
|---|---|---|---|---|
| P3 | QUA-001 | MEDIUM | Medium (2-3 sessions) | Split mai-agent/src/{context,tasks,tools,types}.rs into submodules |
| P4 | QUA-009 | MEDIUM | Small (1 session) | Extract helper functions from deeply nested adapter Python code |
| P5 | QUA-008 | LOW | Medium | Domain-split mai-agent/src/types.rs and mai-api/src/auth.rs |
| P6 | REV-007 | LOW | Small | Audit mai-core/power/ and mai-api/types.rs duplication; expected for HIL drivers |

No blocking or high-priority findings. All four open items are code-quality/style concerns recognized as non-blocking for the RC2 → appliance deployment ladder.

---

## Integrity Baseline

| Gate | Result |
|---|---|
| cargo check --workspace | Pass (zero errors, 0.58s) |
| Working tree git status | Clean |
| HEAD commit date | 2026-05-25 (today) |
| May 2026 commit volume | 142 commits |
| Pre-commit integrity hook | Installed (.integrity/hooks/) |
| .gitignore coverage | .env, 
ode_modules, dist, uild present |

---

## Lane Status (as of Memorial Day)

| Lane | Status | Dates |
|---|---|---|
| **Mainline (Sessions 1–46)** | Complete | 2026-05-15 .. 2026-05-23 |
| **Trust Manifold (BF-1..BF-7)** | Complete | 2026-05-22 .. 2026-05-23 |
| **Ship Hardening (SHIP-01..SHIP-17)** | Complete | 2026-05-23 |
| **RC1 (RC-01..RC-10)** | Shipped | 2026-05-23 .. 2026-05-24 |
| **DOUGHERTY Remediation (J-01..J-26)** | Active | 2026-05-24 .. present |
| **RC2 → Appliance Ladder** | Pending | After DOUGHERTY closure |

---

## Conclusion

The mai/ monorepo scores **93/100** on the Memorial Day 2026 comprehensive static audit. All security, performance, config, project-hygiene, and testing categories pass at 100%. Four low-to-medium findings remain in code quality and review integrity — none blocking, all triaged and mapped to the ongoing DOUGHERTY remediation lane or deferred to the RC2 sprint window.

The working tree is clean, compiles without error, and shows 142 commits of active development during May 2026. The project is in healthy position for the forthcoming RC2 re-bundle.

---

*Report generated by `tools/local_gitdoctor_scan.py` — offline static audit v2*
*Document: mai/docs/MEMORIAL-DAY-SCAN-REPORT.md*
