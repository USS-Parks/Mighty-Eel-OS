# GitDoctor 75 Prompt Roster

> **STATUS as of 2026-05-26 — IN-FLIGHT**
> Landed: GD75-07 (`24a6700`), GD75-08 (`a121d4a`), GD75-09 + GD75-10 (`28f0386`), GD75-14 + GD75-15 (`23b876c`). Remaining: GD75-01..06, GD75-11..13, GD75-16. See [`GITDOCTOR-75-REMEDIATION-PLAN.md`](GITDOCTOR-75-REMEDIATION-PLAN.md) for the per-session plan and dependency graph.

> **Repository identity update (2026-07-16):** the canonical repository is now
> `USS-Parks/Mighty-Eel-OS`. The historical source-artifact filename below retains
> the former repository name so the referenced evidence remains identifiable.

**Lane:** GITDOCTOR-75 (`GD75-01` through `GD75-16`)
**Companion plan:** `docs/GITDOCTOR-75-REMEDIATION-PLAN.md`
**Source artifact:** `docs/USS-Parks-im-mighty-eel-mai-analysis 5.24.2026 - 6_57_PM_PST.pdf`
**Authoring rule:** Each prompt is self-contained for a fresh agent. Read the companion plan first, use a separate worktree, and do not edit unrelated dirty files.

---

## Session Index

| ID | Title | Workstream | Depends on | Effort |
|---|---|---|---|---|
| GD75-01 | Baseline hard-fail reconciliation | W1/W5 | none | S |
| GD75-02 | Test assertion audit and scanner-friendly fixes | W1 | GD75-01 | M |
| GD75-03 | E2E and live-integration manifest | W1/W4 | GD75-01 | M |
| GD75-04 | Dependency lock and offline verification closure | W1/W5 | GD75-01 | M |
| GD75-05 | Adapter input validation layer | W2 | GD75-02 | M-L |
| GD75-06 | Public error redaction pass | W2 | GD75-05 | M |
| GD75-07 | Adapter timeout and connection pooling audit | W3 | GD75-05 | M |
| GD75-08 | Adapter client lifecycle fixes | W3 | GD75-07 | M-L |
| GD75-09 | Local adapter load-balancing design | W3 | GD75-07 | M |
| GD75-10 | Batch capability and request batching pass | W3 | GD75-09 | M-L |
| GD75-11 | Health monitoring and metrics evidence | W4 | GD75-03 | M |
| GD75-12 | Rate-limit and resource-exhaustion coverage | W4 | GD75-03 | M |
| GD75-13 | OpenAPI and contributor entry map | W4/W6 | GD75-03 | M |
| GD75-14 | Response caching policy decision | W3/W6 | GD75-05 | S-M |
| GD75-15 | Local scanner parity and evidence pack | W6 | GD75-01..GD75-14 | M |
| GD75-16 | External rescan, response memo, RC2 handoff | W6 | GD75-15 | S |

---

## Shared Instructions For Every Session

1. Start in a dedicated worktree and branch. Do not commit from the main checkout while other session worktrees are active.
2. Read `docs/GITDOCTOR-75-REMEDIATION-PLAN.md`, `docs/CONCURRENT-SESSIONS.md`, and the files named in the prompt.
3. Preserve MAI's air-gapped, localhost-only appliance design. Do not add external network requirements or broad cloud assumptions to satisfy generic scanner preferences.
4. If a finding is already fixed, document the evidence instead of rewriting the fix.
5. If a finding is a false positive or architecture mismatch, write a crisp refutation with file references and commands.
6. Run the narrowest meaningful verification in-session. Record any skipped full gate with a concrete reason.
7. For new files over 40 lines, follow the workspace staged-write and read-back protocol.

---

## GD75-01: Baseline Hard-Fail Reconciliation

**Workstreams:** W1, W5
**Depends on:** none
**Files in play:** `docs/GITDOCTOR-75-REMEDIATION-PLAN.md`, `docs/LOCAL-GITDOCTOR-REPORT.md`, `.env.example`, `.gitignore`, `Cargo.lock`, `requirements-lock.txt`, possible `package-lock.json`

### Context

The external PDF lists five static-analysis failures: CFG-004, TST-004, TST-005, PRJ-002, and PRJ-004. Local state may already contain fixes that were not in the scanned commit. This session establishes truth before implementation sessions start.

### Prompt

SESSION GD75-01: Reconcile the latest GitDoctor hard failures against the current repo.

IMPLEMENT:
1. Identify the exact current commit and dirty files.
2. Verify whether `.env.example`, `.gitignore`, `Cargo.lock`, `requirements-lock.txt`, and any Node lock file exist.
3. Summarize the current test layout: unit tests, integration tests, e2e tests, live-backend tests, SDK tests.
4. Create `docs/GITDOCTOR-75-BASELINE.md` with a table for the five hard fails: PDF status, current repo status, evidence files, and next owner session.
5. Do not change functional code.

VERIFY:
- Read back the last 5 lines and line count for the new baseline doc.
- Run a quick file-presence check for the artifacts above.

ACCEPTANCE:
- `docs/GITDOCTOR-75-BASELINE.md` exists.
- Every hard static fail is classified as fixed, stale, real, or needs follow-up.

---

## GD75-02: Test Assertion Audit and Scanner-Friendly Fixes

**Workstream:** W1
**Depends on:** GD75-01
**Files in play:** `tests/`, `adapters/*/tests/`, `mai-sdk-python/tests/`, `tools/local_gitdoctor_tests/`, docs evidence

### Context

The PDF flags TST-004, "Test files without assertions." The repo now has many assertion-rich tests, but scanners often count helper files, fixtures, smoke tests, or live-backend skip guards incorrectly.

### Prompt

SESSION GD75-02: Close or refute the "test files without assertions" finding.

IMPLEMENT:
1. Write or update a small local audit tool if one already exists under `tools/` for assertion counting. It should classify Python, Rust, and helper-only files.
2. List all test files with zero direct assertions and classify each as helper, fixture, smoke subprocess wrapper, intentionally skip-only live test, or real gap.
3. For real gaps, add meaningful assertions. Do not add token assertions that only appease the scanner.
4. Document results in `docs/GITDOCTOR-75-ASSERTION-AUDIT.md`.

VERIFY:
- Run the assertion audit.
- Run the touched test files.

ACCEPTANCE:
- No real test file remains without an assertion or explicit helper classification.
- The audit doc gives an external reviewer a scanner-friendly explanation.

---

## GD75-03: E2E and Live-Integration Manifest

**Workstreams:** W1, W4
**Depends on:** GD75-01
**Files in play:** `tests/e2e/`, `tests/sdk_integration.py`, `adapters/*/tests/test_integration_live.py`, `pyproject.toml`, docs evidence

### Context

The PDF flags TST-005, "No integration or e2e tests." Local files indicate e2e and live-backend tests exist, but the scanner may not recognize marker names or optional live suites.

### Prompt

SESSION GD75-03: Make integration and e2e coverage obvious to humans and scanners.

IMPLEMENT:
1. Audit existing e2e, SDK integration, adapter mock integration, and adapter live-backend tests.
2. Ensure pytest markers are declared and clear: `integration`, `e2e`, `live_backend`, and any existing equivalents.
3. Add a minimal full-pipeline smoke if the repo lacks one that exercises auth, health, models, inference or a deterministic mock, and error mapping.
4. Create `docs/GITDOCTOR-75-INTEGRATION-MANIFEST.md` explaining how to run each suite and what requires live hardware/backends.

VERIFY:
- Run `python -m pytest tests/e2e -v` if feasible.
- Run at least one adapter mock integration suite.
- Do not require live GPU/backends for default verification.

ACCEPTANCE:
- The repo has discoverable e2e/integration tests.
- The manifest names skip conditions so optional live tests do not look absent.

---

## GD75-04: Dependency Lock and Offline Verification Closure

**Workstreams:** W1, W5
**Depends on:** GD75-01
**Files in play:** `Cargo.lock`, `requirements-lock.txt`, `pyproject.toml`, `.integrity/mcp-server/package.json`, possible `package-lock.json`, docs

### Context

The PDF flags PRJ-004, "Missing dependency lock file," and a security narrative finding says missing lock files make dependency tampering harder to detect. Local `Cargo.lock` and `requirements-lock.txt` appear to exist. Node tooling needs a deliberate decision.

### Prompt

SESSION GD75-04: Close dependency lock and offline verification findings.

IMPLEMENT:
1. Verify `Cargo.lock` is present and current for the Rust workspace.
2. Verify `requirements-lock.txt` contains hashes and matches the intended Python dependency surface.
3. Inspect `.integrity/mcp-server/package.json`. If it has external runtime dependencies, generate and commit a lock file. If not, document why no Node lock is required.
4. Add `docs/DEPENDENCY-LOCK-POLICY.md` or update an existing equivalent with Rust, Python, and Node/tooling policy.
5. Add a short note to the GitDoctor evidence doc if it already exists.

VERIFY:
- Run the relevant lock check commands available in the repo.
- Do not fetch network dependencies unless explicitly allowed.

ACCEPTANCE:
- Every package ecosystem has a committed lock file or a written no-lock rationale.
- Hash verification for Python is documented.

---

## GD75-05: Adapter Input Validation Layer

**Workstream:** W2
**Depends on:** GD75-02
**Files in play:** `adapters/base.py`, `adapters/*/adapter.py`, `adapters/*/tests/`

### Context

The PDF reports "Missing Input Sanitization" for `adapters/*/adapter.py`. MAI should validate shape, size, and supported features without mutating or censoring legitimate prompts.

### Prompt

SESSION GD75-05: Add a shared adapter input validation contract.

IMPLEMENT:
1. Inspect `AdapterBase` and concrete adapter generation/chat/embed entry points.
2. Add a shared validation helper or base-class method for prompt/message shape, empty inputs, overlarge inputs, invalid roles, negative token limits, invalid sampling ranges, and unsupported multimodal payloads.
3. Wire concrete adapters through the shared validation point.
4. Add tests for at least three adapter families plus base-level tests.
5. Keep validation deterministic and local; do not add content moderation or network calls.

VERIFY:
- Run touched adapter tests.
- Run type/lint checks if local tooling supports them.

ACCEPTANCE:
- Malformed inputs fail before backend calls.
- Tests prove validation does not break valid prompts.

---

## GD75-06: Public Error Redaction Pass

**Workstream:** W2
**Depends on:** GD75-05
**Files in play:** `adapters/base.py`, adapter error mapping, `mai-api` public error serializers, tests

### Context

The PDF warns that error messages may leak internal path information to clients. This is especially relevant on Windows paths and temp staging paths.

### Prompt

SESSION GD75-06: Prevent public errors from leaking local filesystem paths.

IMPLEMENT:
1. Identify public error serialization paths in adapters and API handlers.
2. Add or reuse a redaction helper for Windows paths, POSIX paths, temp paths, and home-directory paths.
3. Preserve full diagnostic detail in internal logs/audit where appropriate, but scrub client-facing messages.
4. Add regression tests covering Windows and POSIX path examples.
5. Document the boundary in an existing security or API doc.

VERIFY:
- Run targeted adapter/API tests.

ACCEPTANCE:
- Client-visible errors do not include absolute host paths.
- Internal diagnostics remain useful.

---

## GD75-07: Adapter Timeout and Connection Pooling Audit

**Workstream:** W3
**Depends on:** GD75-05
**Files in play:** `adapters/*/client.py`, `adapters/*/config.py`, docs

### Context

The PDF reports missing timeout handling, magic timeout/retry numbers, duplicate HTTP client patterns, and lack of connection pooling.

### Prompt

SESSION GD75-07: Audit adapter HTTP timeout and pooling behavior.

IMPLEMENT:
1. Inspect every `adapters/*/client.py` and `adapters/*/config.py`.
2. Build a matrix: client library used, client lifetime, connect timeout, request timeout, stream timeout, retry behavior, shutdown behavior, pooling status.
3. Classify fixes by risk: safe quick fixes, larger lifecycle refactors, and intentional no-op.
4. Write `docs/GITDOCTOR-75-ADAPTER-HTTP-AUDIT.md`.
5. Do not refactor all clients in this session unless the change is tiny and obvious.

VERIFY:
- Run no broad tests unless code changes are made.

ACCEPTANCE:
- The next implementation session has an exact change list and no guesswork.

---

## GD75-08: Adapter Client Lifecycle Fixes

**Workstream:** W3
**Depends on:** GD75-07
**Files in play:** `adapters/*/client.py`, `adapters/*/adapter.py`, tests

### Context

Use the audit from GD75-07 to implement safe pooling and lifecycle fixes. The goal is reusable local HTTP clients with explicit close behavior, not new remote connectivity.

### Prompt

SESSION GD75-08: Implement safe adapter HTTP client lifecycle improvements.

IMPLEMENT:
1. Apply the safe fixes identified in `docs/GITDOCTOR-75-ADAPTER-HTTP-AUDIT.md`.
2. Prefer one persistent async client per adapter instance where the backend protocol supports it.
3. Ensure `shutdown` or async context-manager cleanup closes clients.
4. Keep timeout settings in config with clear defaults.
5. Add tests proving client reuse and close behavior.

VERIFY:
- Run all touched adapter tests.
- Run at least one mock integration suite.

ACCEPTANCE:
- HTTP clients are reused where safe.
- Shutdown closes owned resources.
- No adapter starts talking to non-local hosts by default.

---

## GD75-09: Local Adapter Load-Balancing Design

**Workstream:** W3
**Depends on:** GD75-07
**Files in play:** `mai-scheduler/`, `mai-adapters/`, `mai-api/`, docs

### Context

The PDF asks for adapter load balancing. MAI should interpret this as local appliance routing across multiple local instances of the same backend, not horizontal cloud clustering.

### Prompt

SESSION GD75-09: Design local-only adapter load balancing.

IMPLEMENT:
1. Inspect scheduler and adapter manager capabilities.
2. Determine whether current topology can represent multiple instances of the same adapter/backend.
3. Draft `docs/LOCAL-ADAPTER-LOAD-BALANCING.md` with routing inputs, health weighting, backpressure, affinity, and air-gap constraints.
4. If a small code hook is clearly missing, add a minimal data structure or TODO-free interface, but avoid speculative implementation.

VERIFY:
- Run docs checks if available.
- Run targeted Rust tests if code changed.

ACCEPTANCE:
- The design can be implemented without weakening localhost-only policy.
- The evidence pack can classify generic multi-node scaling as intentional out of scope.

---

## GD75-10: Batch Capability and Request Batching Pass

**Workstream:** W3
**Depends on:** GD75-09
**Files in play:** `adapters/base.py`, adapter capability declarations, scheduler batching, tests, docs

### Context

The PDF notes limited batch processing. MAI should expose batching where the backend supports it, especially embeddings and compatible local inference servers.

### Prompt

SESSION GD75-10: Clarify and improve batch processing capabilities.

IMPLEMENT:
1. Audit `AdapterCapabilities` and each adapter's declared batching support.
2. Add tests that capability declarations match implemented behavior.
3. Implement a conservative batch path for one high-value backend if the current API already supports it cleanly.
4. Document unsupported batch paths explicitly.

VERIFY:
- Run base adapter tests and touched adapter tests.

ACCEPTANCE:
- Batch support is accurate, tested, and no longer looks accidentally absent.

---

## GD75-11: Health Monitoring and Metrics Evidence

**Workstream:** W4
**Depends on:** GD75-03
**Files in play:** `mai-api/src/handlers/`, `mai-api/src/routes.rs`, adapter health code, docs

### Context

The PDF recommends comprehensive health checking and metrics. The repo likely has health endpoints, but the evidence must be obvious.

### Prompt

SESSION GD75-11: Make health monitoring externally reviewable.

IMPLEMENT:
1. Inventory health endpoints and adapter health signals.
2. Add or improve a system health aggregation test if missing.
3. Ensure the response distinguishes healthy, degraded, and unavailable components.
4. Document operator health checks in `docs/OBSERVABILITY.md` or a linked GitDoctor evidence doc.

VERIFY:
- Run targeted API health tests.

ACCEPTANCE:
- External reviewers can find and exercise health monitoring without reading internals.

---

## GD75-12: Rate-Limit and Resource-Exhaustion Coverage

**Workstream:** W4
**Depends on:** GD75-03
**Files in play:** `mai-api/src/auth.rs`, route middleware, tests, docs

### Context

The PDF recommends request rate limiting to prevent resource exhaustion. The earlier scans also mentioned rate-limit and schema-validation checks.

### Prompt

SESSION GD75-12: Prove or add rate-limit/resource-exhaustion controls.

IMPLEMENT:
1. Inspect existing auth and middleware for rate limits, quotas, request-size limits, and timeout behavior.
2. If controls exist, add tests and docs so scanners/reviewers can see them.
3. If controls are missing, add a conservative local rate-limit guard appropriate for an appliance API.
4. Ensure errors use the typed error hierarchy and include retry metadata when applicable.

VERIFY:
- Run targeted API middleware tests.

ACCEPTANCE:
- Resource-exhaustion protections are test-backed and documented.

---

## GD75-13: OpenAPI and Contributor Entry Map

**Workstreams:** W4, W6
**Depends on:** GD75-03
**Files in play:** `docs/API-REFERENCE.md`, route definitions, possible OpenAPI artifact, `docs/README-FIRST.md`

### Context

The PDF says documentation is rich but missing comprehensive API documentation, and the domain may be hard for new contributors.

### Prompt

SESSION GD75-13: Improve API and contributor discoverability.

IMPLEMENT:
1. Determine whether OpenAPI generation already exists. If yes, update and document it. If no, create a maintained `docs/openapi.json` or `docs/OPENAPI.md` contract from current routes.
2. Link the artifact from `docs/API-REFERENCE.md`.
3. Add a compact contributor entry map: repo structure, core flows, where to add tests, and how to run the main gates.
4. Keep docs factual and aligned with current code.

VERIFY:
- Run any doc generation command if added.
- Validate JSON/YAML if an OpenAPI artifact is created.

ACCEPTANCE:
- A reviewer can inspect the REST contract without crawling route code.
- New-contributor complexity is reduced with a short entry map.

---

## GD75-14: Response Caching Policy Decision

**Workstreams:** W3, W6
**Depends on:** GD75-05
**Files in play:** docs first; code only if policy is safe and small

### Context

The PDF recommends response caching. For MAI, naive prompt-response caching can conflict with sensitive data handling and audit expectations.

### Prompt

SESSION GD75-14: Decide response caching policy safely.

IMPLEMENT:
1. Analyze caching by endpoint type: chat/completion, embeddings, models, health, audit, compliance reports.
2. Classify which responses are safe to cache, which are unsafe, and which might be safe with TTL/auth/data-class constraints.
3. Write `docs/RESPONSE-CACHING-POLICY.md`.
4. Implement only low-risk caching if it is clearly safe, such as static model metadata, and only with tests.

VERIFY:
- Run touched tests if code changes.

ACCEPTANCE:
- The evidence pack can answer the caching tip without unsafe over-implementation.

---

## GD75-15: Local Scanner Parity and Evidence Pack

**Workstream:** W6
**Depends on:** GD75-01 through GD75-14
**Files in play:** `tools/local_gitdoctor*`, `docs/GITDOCTOR-75-EVIDENCE.md`, `docs/GITDOCTOR-75-EVIDENCE.json`

### Context

Before asking for another external scan, MAI needs one concise evidence packet mapping every PDF finding to fixed/refuted/deferred status.

### Prompt

SESSION GD75-15: Build the GitDoctor 75 evidence pack.

IMPLEMENT:
1. Review outputs from GD75-01 through GD75-14.
2. Update local GitDoctor-style scanner checks only if they are stale or noisier than the external report. Do not game the scanner.
3. Create `docs/GITDOCTOR-75-EVIDENCE.md` with a finding-by-finding closure matrix.
4. Create `docs/GITDOCTOR-75-EVIDENCE.json` with machine-readable finding IDs, status, files, commands, and owner session.
5. Include false positives and architecture tradeoffs explicitly.

VERIFY:
- Run the local scanner if available.
- Run JSON validation for the evidence file.

ACCEPTANCE:
- Every PDF item has a status and proof path.
- The evidence pack is short enough for an outside reviewer to use.

---

## GD75-16: External Rescan, Response Memo, RC2 Handoff

**Workstream:** W6
**Depends on:** GD75-15
**Files in play:** `docs/GITDOCTOR-75-RESCAN-NOTES.md`, `docs/HANDOFF.md`, release docs if appropriate

### Context

This closes the lane by capturing the final external result and handing the repo back to the RC2 deployment ladder.

### Prompt

SESSION GD75-16: Run or prepare external rescan and close the lane.

IMPLEMENT:
1. Prepare the exact commit/branch for external scanning.
2. Run the external scan if credentials/process are available, or write the exact rescan instructions if a human must run it.
3. Create `docs/GITDOCTOR-75-RESCAN-NOTES.md` with old score, new score, unresolved items, and rationale.
4. Draft a concise reviewer response memo summarizing fixes, false positives, and remaining intentional architecture tradeoffs.
5. Update handoff/release docs only if this lane is truly ready to feed RC2.

VERIFY:
- Confirm evidence links resolve.
- Run the final agreed subset of project gates and record results.

ACCEPTANCE:
- The lane has a final score or ready-to-run rescan packet.
- RC2 handoff knows exactly what remains.
