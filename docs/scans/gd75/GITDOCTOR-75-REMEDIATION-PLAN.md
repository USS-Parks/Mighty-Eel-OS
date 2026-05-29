# GitDoctor 75 Remediation Plan

> **STATUS as of 2026-05-26 — IN-FLIGHT**
> Sessions landed on `origin/main`: GD75-07 (`24a6700` adapter health-check timeouts + pooling), GD75-08 (`a121d4a` pooled HTTP client lifecycle), GD75-09/10 (`28f0386` local load-balancing + batching matrix), GD75-14/15 (`23b876c` caching policy + evidence pack). Remaining: GD75-01..06, GD75-11..13, GD75-16. DOUGHERTY (predecessor) closed 2026-05-25. Next milestone is GD75-16 (external rescan + RC2 handoff). Latest local scan: [`LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md`](../LOCAL-GITDOCTOR-EVIDENCE-2026-05-26.md).

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Lane:** GITDOCTOR-75, sessions `GD75-01` through `GD75-16`
**Plan date:** 2026-05-25
**Source artifact:** `docs/USS-Parks-im-mighty-eel-mai-analysis 5.24.2026 - 6_57_PM_PST.pdf`
**Source generated date:** 2026-05-24, 6:57 PM Pacific time in the filename
**External score:** Overall 75/100, Vibe 80/100, Production 70/100
**Target:** reach 95+/100 on the next external scan, with a documented path to 100/100 where the final points do not conflict with MAI's air-gapped appliance architecture.
**Relationship to DOUGHERTY:** This lane succeeds and tightens the John Dougherty remediation lane. It should not rewrite or erase the DOUGHERTY evidence trail.

---

## 1. Executive Read

The latest external report is materially better than the first outside review. It describes MAI as a sophisticated Rust/Python inference middleware system with professional architecture, strong error handling, good CI/CD, clear adapter boundaries, and an air-gap security posture. The scan no longer reports critical findings, and all 16 static security checks pass.

The remaining score loss comes from three buckets:

1. **Hard static-analysis failures:** five checks still failed in the PDF: `.env.example`, test assertions, integration/e2e tests, `.gitignore`, and dependency lock files.
2. **Narrative production-readiness gaps:** input validation, dependency verification, path disclosure in errors, connection pooling, load balancing, health monitoring, OpenAPI docs, rate limiting, caching, batching, and new-contributor complexity.
3. **Architecture mismatch:** the report rewards generic horizontally scalable web-service patterns, while MAI intentionally prioritizes single-node, localhost-only, air-gapped appliance deployment. Some points should be won with evidence and docs, not by weakening the product shape.

Local state note: at the time this plan was written, the working tree already contained uncommitted changes that appear to close some PDF findings, including `.env.example`, `.gitignore` coverage, `requirements-lock.txt`, live adapter integration tests, and Rust SDK HTTP tests. `GD75-01` exists to verify those facts against the actual branch before more code changes begin.

---

## 2. Score Snapshot

| Area | PDF score/status | Plan response |
|---|---:|---|
| Overall | 75/100 | Push to 95+ by closing five hard static fails and strengthening evidence. |
| Vibe | 80/100, Clean & Professional | Preserve. Avoid overfitting or broad churn. |
| Production | 70/100, Nearly Ready | Focus on reproducibility, live tests, validation, and operator docs. |
| Code Quality | 78/100 | Address duplicate adapter HTTP patterns and magic numbers only where they reduce risk. |
| Error Handling | 85/100 | Add public-error redaction and timeout coverage evidence. |
| Security | 82/100 narrative, 16/16 static passed | Strengthen validation, rate limits, and dependency verification. |
| Testing | 70/100, 4/6 static passed | Close assertion and e2e/integration findings with measurable gates. |
| Documentation | 75/100 | Add OpenAPI generation and a contributor entry map. |
| Architecture | 83/100 | Document air-gap tradeoffs; implement local load balancing where useful. |
| Scalability | 65/100 | Improve single-node adapter balancing and batching without multi-node drift. |
| DevOps | 78/100 | Lock files, `.env.example`, `.gitignore`, Docker/container stance evidence. |
| UI/UX | 60/100 | Treat as non-core. Only compliance/operator UI is in scope. |
| Frontend Performance | 50/100 | Treat as non-core unless the compliance dashboard is touched. |

---

## 3. Hard Static Failures

These are the highest-confidence items because they are explicitly listed as failed checks in the PDF.

| ID | PDF finding | Current local read | Verdict | Session |
|---|---|---|---|---|
| CFG-004 | Missing `.env.example` | `.env.example` exists locally and is detailed. | Likely fixed locally; verify against committed branch and external scan input. | GD75-01 |
| TST-004 | Test files without assertions | Many tests now include assertions, but the external scanner may still flag fixtures, live skips, or helper tests. | Needs assertion audit and scanner-friendly evidence. | GD75-02 |
| TST-005 | No integration or e2e tests | `tests/e2e/`, `tests/sdk_integration.py`, and adapter live integration tests exist locally. | Likely fixed locally; verify markers and CI exposure. | GD75-03 |
| PRJ-002 | Incomplete `.gitignore` | `.gitignore` includes `node_modules/`, `*.log`, Python, Rust, OS, env, and test artifacts. | Likely fixed locally; verify branch and scanner input. | GD75-01 |
| PRJ-004 | Missing dependency lock file | `Cargo.lock` and `requirements-lock.txt` exist locally; Node lock may be intentionally absent or still needed for `.integrity/mcp-server`. | Partially fixed; close Node/tooling lock decision explicitly. | GD75-04 |

Acceptance rule: if a hard fail is already fixed locally, do not rework it. Commit or document the existing fix, add evidence, and move on.

---

## 4. Narrative Findings

These issues were not all hard static failures, but they cost points and may hide real production risk.

| Theme | PDF wording | Risk | Plan action |
|---|---|---|---|
| Input sanitization | "Some adapter endpoints may not properly sanitize prompt inputs" | Malformed prompt payloads could reach backends inconsistently. | Central adapter input validation and per-adapter tests. |
| Dependency verification | "Missing lock files make dependency tampering harder to detect" | Reproducibility and supply-chain trust. | Lock policy, hash verification, cargo/npm/pip audit evidence. |
| Error disclosure | "Some error messages may leak internal path information" | Public API may reveal host paths or implementation details. | Redaction helper and regression tests. |
| Connection pooling | "HTTP clients create new connections for each request" | Avoidable adapter latency and socket churn. | Audit clients, preserve reusable async clients, add lifecycle tests. |
| Load balancing | "Add basic load balancing across multiple instances" | Useful on a single appliance with multiple local workers. | Local-only adapter pool routing; no remote cluster dependency. |
| Health monitoring | "Comprehensive health checking and metrics" | Operators need fast triage. | Verify existing health endpoint, add adapter/system aggregation evidence. |
| OpenAPI docs | "Missing comprehensive API documentation" | Lowers reviewer confidence and SDK alignment. | Generate and check OpenAPI artifact, link from API docs. |
| Rate limiting | "Prevent resource exhaustion attacks" | Local misuse can starve GPU/CPU resources. | Verify auth/rate-limit behavior and add coverage where missing. |
| Response caching | "Intelligent caching for repeated inference requests" | Can improve repeated deterministic calls, but can leak sensitive prompts if naive. | Policy-first design; implement only safe, disabled-by-default cache or document deferral. |
| Batching | "Limited batch processing capabilities" | Throughput gap for embeddings and compatible backends. | Audit adapter capabilities, add batch contract where backend supports it. |
| Complexity | "Complex domain may be difficult for new contributors" | Slower buyer/acquirer technical diligence. | Contributor map and "first 30 minutes" doc. |

---

## 5. False-Positive and Architecture-Gap Ledger

MAI should not mutate into a generic cloud web app just to satisfy a scanner. Each session that touches one of these areas must preserve the intent below.

| Scanner concern | Classification | Required response |
|---|---|---|
| Limited horizontal scalability | Intentional architecture tradeoff. | Document single-node appliance design, local multi-adapter scaling, and acquirer-owned remote orchestration path. |
| Hardcoded localhost URLs | Intentional air-gap enforcement, not a smell. | Keep localhost-only defaults; make ship profile docs explicit. |
| UI/UX and frontend performance | Mostly not applicable; MAI is a backend appliance plus limited operator/compliance surfaces. | Do not add a broad web dashboard unless it supports existing operator/compliance flows. |
| Response caching | Potentially risky with sensitive prompts. | Default to no prompt caching unless data classification, TTL, auth scope, and audit semantics are defined. |
| Adding dependencies to improve convenience | Risky for air-gap and supply-chain posture. | Prefer stdlib or already-approved dependencies unless there is a clear security/reliability gain. |
| Multi-node load balancing | Out of scope for core MAI RC path. | If implemented, keep first pass local-only across multiple backend instances on the same appliance. |

False-positive handling rule: every dismissed finding needs a short evidence note in the final evidence pack. Silence looks like neglect to external reviewers.

---

## 6. Workstreams

### W1. Scanner Hard-Fail Closure

Close or prove closure of CFG-004, PRJ-002, PRJ-004, TST-004, and TST-005. This is the fastest path from 75 toward the high 80s or low 90s.

Deliverables:
- Static-fail closure table with file references and commands.
- Branch/commit evidence for `.env.example`, `.gitignore`, and lock files.
- Assertion-audit output showing which files have real assertions and which are intentionally helpers.
- Integration/e2e manifest that distinguishes mock integration, live-backend optional integration, and full server smoke.

### W2. Adapter Input and Error Safety

Strengthen adapter entry points so malformed requests fail before reaching backends, and public errors do not leak local host paths.

Deliverables:
- Shared validation helper or explicit validation contract in `adapters/base.py`.
- Tests for empty prompts, overlarge prompts, invalid message roles, unsupported multimodal payloads, invalid sampling ranges, and context-window checks.
- Redaction tests for Windows and POSIX paths.

### W3. Adapter Performance and Lifecycle

Address connection pooling, timeout consistency, duplicate HTTP client patterns, and local batching/load-balancing capability.

Deliverables:
- HTTP client lifecycle audit for each adapter.
- Reused async HTTP clients with explicit shutdown where safe.
- Config defaults for connect/read/stream timeouts.
- Local adapter-pool routing design, with air-gap constraints.
- Batch capability matrix tied to adapter capabilities.

### W4. Production API Evidence

Make the REST contract inspectable and prove operational controls.

Deliverables:
- OpenAPI artifact generated from actual routes or maintained alongside tests.
- Health/rate-limit behavior tests.
- API reference link update.
- E2E smoke that exercises auth, health, model listing, a minimal inference path, and error mapping.

### W5. Dependency and DevOps Evidence

Make reproducibility and offline installation reviewable.

Deliverables:
- Python lock file with hashes.
- Cargo lock and audit notes.
- Node lock decision for `.integrity/mcp-server`.
- `.env.example` and `.gitignore` evidence.
- Container stance: either a CPU-only Dockerfile for evaluation or a documented no-container production rationale plus existing package path.

### W6. Reviewer Confidence and Rescan

Package the closure evidence so the next outside reviewer sees the repo the way we do.

Deliverables:
- GitDoctor 75 evidence pack.
- Local scanner parity update if needed.
- Reviewer response memo.
- Fresh external rescan target and score comparison.

---

## 7. Session Map

| ID | Title | Workstream | Depends on | Expected files | Effort |
|---|---|---|---|---:|---|
| GD75-01 | Baseline hard-fail reconciliation | W1, W5 | none | docs/evidence only | S |
| GD75-02 | Test assertion audit and scanner-friendly fixes | W1 | GD75-01 | tests/tools/docs | M |
| GD75-03 | E2E and live-integration manifest | W1, W4 | GD75-01 | tests/docs/CI | M |
| GD75-04 | Dependency lock and offline verification closure | W1, W5 | GD75-01 | lock/docs/tools | M |
| GD75-05 | Adapter input validation layer | W2 | GD75-02 | adapters/tests | M-L |
| GD75-06 | Public error redaction pass | W2 | GD75-05 | adapters/API/tests | M |
| GD75-07 | Adapter timeout and connection pooling audit | W3 | GD75-05 | adapters/tests/docs | M |
| GD75-08 | Adapter client lifecycle fixes | W3 | GD75-07 | adapters/tests | M-L |
| GD75-09 | Local adapter load-balancing design | W3 | GD75-07 | scheduler/API/docs | M |
| GD75-10 | Batch capability and request batching pass | W3 | GD75-09 | adapters/scheduler/tests | M-L |
| GD75-11 | Health monitoring and metrics evidence | W4 | GD75-03 | API/tests/docs | M |
| GD75-12 | Rate-limit and resource-exhaustion coverage | W4 | GD75-03 | API/tests/docs | M |
| GD75-13 | OpenAPI and contributor entry map | W4, W6 | GD75-03 | docs/tools | M |
| GD75-14 | Response caching policy decision | W3, W6 | GD75-05 | docs/code optional | S-M |
| GD75-15 | Local scanner parity and evidence pack | W6 | GD75-01..GD75-14 | docs/tools | M |
| GD75-16 | External rescan, response memo, RC2 handoff | W6 | GD75-15 | docs/evidence | S |

No session should touch unrelated dirty work from the DOUGHERTY lane. Use a separate worktree and branch per session, following `docs/CONCURRENT-SESSIONS.md`.

---

## 8. Gates

### Gate GD-A: Hard Static Fails Closed

Required before `GD75-05` starts:
- `.env.example` exists and is referenced from setup docs.
- `.gitignore` includes Python, Rust, Node, env, OS, logs, build, and test artifacts.
- Lock-file policy is explicit for Rust, Python, and Node/tooling.
- Assertion audit has no unexplained test files with zero assertions.
- Integration/e2e manifest names every suite, marker, and skip condition.

### Gate GD-B: Safety and Performance Corrections

Required before `GD75-11` starts:
- Adapter validation tests pass.
- Public error messages are redacted.
- Adapter HTTP lifecycle and timeout decisions are documented.
- Any pooling changes have shutdown tests.

### Gate GD-C: Production Evidence Ready

Required before `GD75-15` starts:
- Health and rate-limit behavior covered.
- OpenAPI or API contract evidence generated.
- Batch/load-balancing decisions are either implemented or documented as intentional deferrals.
- Caching decision is explicit and security-reviewed.

### Gate GD-D: Rescan Ready

Required before `GD75-16` starts:
- Evidence pack links every PDF finding to fixed, refuted, or intentionally deferred status.
- Local GitDoctor-style scan has no unexplained regressions.
- Standard project gates have been run or documented as unavailable.

---

## 9. Verification Commands

Use the normal commands when a session touches the relevant area:

```powershell
cargo fmt --check
cargo check --workspace
cargo clippy --workspace -- -D warnings -A clippy::pedantic
cargo test --workspace
python -m pytest
python -m pytest tests/e2e adapters -m "not live_backend"
```

For integrity checks, use Git Bash from `mai/`:

```bash
mai/.integrity/scripts/verify-tree.sh <changed-files>
```

If a command is too expensive or requires missing live hardware/backends, record the exact reason and the narrower command that did run.

---

## 10. Evidence Pack Layout

`GD75-15` should create or update a compact evidence pack. Suggested files:

| File | Purpose |
|---|---|
| `docs/GITDOCTOR-75-EVIDENCE.md` | Human-readable closure matrix. |
| `docs/GITDOCTOR-75-EVIDENCE.json` | Machine-readable finding status, commands, and files. |
| `docs/GITDOCTOR-75-RESCAN-NOTES.md` | External rescan instructions and score comparison. |

The evidence pack should include the original PDF score, the local current score if available, and the final external score after `GD75-16`.

---

## 11. Done Definition

This lane is complete when:

1. Every hard static failure from the 2026-05-24 PDF is fixed or proven stale against the scanned branch.
2. Every narrative issue is fixed, refuted with evidence, or deferred with a security/architecture rationale.
3. The local scanner and standard test gates show no new unexplained regressions.
4. The external reviewer has a short response memo and evidence pack.
5. The next external scan is 95+/100, or any remaining lost points are traced to intentional MAI architecture constraints rather than accidental gaps.

