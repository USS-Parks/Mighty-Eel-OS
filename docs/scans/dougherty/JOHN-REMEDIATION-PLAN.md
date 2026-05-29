# John Dougherty Remediation Plan (DOUGHERTY Lane)

> **STATUS — CLOSED (2026-05-25). SUPERSEDED.**
> This is the original draft of the DOUGHERTY plan. The canonical re-emitted version lives at [`../JOHN-REMEDIATION-PLAN.md`](../JOHN-REMEDIATION-PLAN.md) (top-level `docs/`) and was amended to add J-10b. Lane closed 2026-05-25 with all 27 sessions (J-01..J-26 + J-10b) complete; J-23..J-26 landed under `a072634`. Closure document: [`J-15-DOUGHERTY-CLOSURE.md`](J-15-DOUGHERTY-CLOSURE.md). Current build work is in the GITDOCTOR-75 lane.

**Project:** Island Mountain Model Abstraction Interface (MAI) — Lamprey RC1
**Source Trigger:** Email from John Dougherty (independent tester, Colorado), 2026-05-24
**Trigger Artifact:** GitDoctor scan of `USS-Parks/im-mighty-eel-mai` (15 screenshots in `.tester-feedback-2026-05-24/`)
**Lane Name:** DOUGHERTY (sessions prefixed `J-`)
**Sessions:** J-01 … J-26 (26 sessions). Amended 2026-05-24 to add J-16/J-17 for `mai-sdk-rs` stubs and J-18..J-26 for comprehensive adapter completion. J-14 (rescan) and J-15 (response) keep their numbers; the dependency graph in §4 routes J-14 through J-16..J-26.
**Status of plan:** **CLOSED 2026-05-25** (was ACTIVE 2026-05-24). Superseded by top-level canonical re-emit; kept as historical record.
**Scope guarantee:** This plan addresses every line of John's email AND every flagged issue across all 15 GitDoctor screenshots. Items judged to be false positives are kept in scope as documented refutations, not silently dropped.

---

## 1. Context

### 1.1 What John tested
GitDoctor (gitdoctor.io) AI code-scan service ran a static analysis against the GitHub repository `USS-Parks/im-mighty-eel-mai` (origin/main, which is currently in sync with local HEAD `5be7d2b`). Scores:

| Category         | Score   | Severity |
|:-----------------|:--------|:---------|
| Overall          | 52/100  | Needs Work |
| Vibe Score       | 35/100  | Likely Vibe-Coded (negative) |
| Production Score | 41/100  | Needs Work |
| Code Quality     | 40/100  | — |
| Error Handling   | 60/100  | — |
| Security         | 75/100  | — |
| Testing          | 25/100  | — |
| Documentation    | 85/100  | — |
| Architecture     | 70/100  | — |
| Scalability      | 45/100  | — |
| DevOps Readiness | 65/100  | — |

50 checks run · 41 pass · 9 fail · 0 critical · 3 security findings · 10 tips.

### 1.2 Where this lands in the existing roadmap
The RC1 lane (`project_rc_release_lane`) had RC-09 (First Outside Tester) gated on a real outside human. John IS that human. His email is the RC-09 verdict. The DOUGHERTY lane is the response — it sits **between RC-09 (received) and RC-10 (re-bundle + ship)** in the release sequence.

```
RC-08 (bundle) → RC-09 (tester verdict: John) → [DOUGHERTY LANE: J-01..J-26] → RC-10 (re-bundle) → RC-11 (re-ship)
```

The Ship Hardening lane is closed; the mainline plan is closed (Gate D, S46). DOUGHERTY is the only active lane until RC-10 opens.

### 1.3 Co-author and integrity rules carry forward
Every commit produced under this lane ends with the canonical single-line co-author footer (`feedback_commit_coauthor`). Every session ≥3 files triggers the post-write subagent verification per workspace `CLAUDE.md`. Every new file >40 lines goes through the two-stage staged-write protocol. No exceptions for hygiene-class sessions.

---

## 2. Triage of every John issue

Every claim from John's email plus every GitDoctor flag is in this table. Each row carries a verdict from re-reading the actual filesystem this session, not from assumption.

| # | Source | Claim | GitDoctor ID | Real? | Owner | Severity | Workstream |
|:--|:--|:--|:--|:--|:--|:--|:--|
| 1 | email + QUA-004 | "Extensive TODO placeholders and incomplete implementations throughout" | QUA-004 + Code Smell `Placeholder Implementation` HIGH | Mixed — TRUE for `mai-sdk-rs/src/lib.rs` (17 `todo!()` sites at lines 768-887, see row 1b); TRUE in `.integrity/mcp-server/server.js`; FALSE for `adapters/*/adapter.py` (Ollama 316 LOC, llama.cpp 273, exllamav2 289, vllm 332, tgi 233, sglang 249, tensorrt 253 — zero `NotImplementedError`, zero trailing `pass`) | mixed | P0 refutation (adapters) + P1 fix (SDK) + P2 mcp-server clean | W1, W8, W10 |
| 1b | direct review (Basho-flagged 2026-05-24) | "mai-sdk-rs/src/lib.rs still has a block of todo!() HTTP-client methods" | n/a — GitDoctor scanned but did not surface (Rust `todo!()` macro not in its heuristic set) | TRUE — 17 stubs across HTTP + SSE + resume protocol, no `tests/` directory, no `reqwest` in Cargo.toml. Previously tracked as SHIP-17 `KNOWN-ISSUES.md` Issue 15 ("no in-tree consumer, not lane-blocking") — true at the time, but John-visible to any reviewer who opens the SDK file, so refutation-blocking. | mai-sdk-rs | P1 | W10 |
| 2 | email + Error Handling 60 | "Error mapping designed but not consistently applied" | Error Handling 60/100 | Needs audit — assume real in places | mai-api, adapters | P1 | W4 |
| 3 | email | "Uses stdlib only to avoid supply chain risks. Needs to be improved" | Security 75 commentary | FALSE — stdlib-only is intentional air-gap design; pulling third-party crates is a regression, not a fix | mai-* | refute | W8 |
| 4 | email + TST-001/005/006 | "Most tests are minimal stubs with mocked responses" | TST-001 + TST-005 + TST-006 | Partial — ollama tests have 38 assertions (real); llamacpp has 14, exllamav2 has 13 (thin); compliance demos are real | adapter tests | P1 | W3, W5 |
| 5 | email | "Complete Core Adapter Implementation, start with Ollama" | Tip + Code Smell `Placeholder Implementation` | FALSE — Ollama adapter is the Session 08 deliverable, full body, full test coverage | n/a | refute with evidence | W8 |
| 6 | email + SEC-009 | "Math.random() in security-sensitive contexts" | SEC-009 HIGH | TRUE — `.integrity/mcp-server/server.js:233` uses `Math.random().toString(36)` to mint a tmp filename for staged writes | mcp-server | P0 | W1 |
| 7 | email + CFG-007 | "Add Docker Configuration … multi-stage builds" | CFG-007 LOW + Tip HIGH | TRUE — no Dockerfile in tree; deployment is bare-metal-only today | packaging | P1 | W2 |
| 8 | email + TST-005 | "Write REAL integration tests against real backends" | TST-005 MEDIUM + Tip | Partial — there are real Rust integration tests (compliance demos, perf); no live-backend Python adapter integration suite | adapter tests | P2 | W3 |
| 9 | email | "Add HTTP connection pooling to adapter clients" | Tip MEDIUM | Likely real — `adapters/*/client.py` to be audited; needs measurement before fixing | adapters | P2 | W3 |
| 10 | email + PRJ-004 | "Add Cargo.lock and requirements-lock.txt or equivalent" | PRJ-004 HIGH + Tip | Partial — `Cargo.lock` exists; missing `requirements*.txt`/`uv.lock`/`poetry.lock` for Python and missing `package-lock.json` for `.integrity/mcp-server/` | packaging | P1 | W2 |
| 11 | email + TST-004 | "Test files without assertions … add meaningful assertions" | TST-004 HIGH | Partial — true for thin adapter tests (llamacpp/exllamav2 stubs); false for ollama and compliance suites | adapter tests | P2 | W5 |
| 12 | email | "Async context managers for adapter lifecycle" | Tip MEDIUM | Real — adapters don't expose `__aenter__`/`__aexit__`; lifecycle is via `initialize`/`shutdown` | adapters | P3 | W7 |
| 13 | email | "Health check aggregator for production monitoring" | Tip + CFG-003 (no `/health` endpoint) | Real — per-adapter health exists; no aggregator | mai-api | P2 | W7 |
| 14 | email | "Simple web dashboard for monitoring adapter status" | Tip | **Conflict with arch** — CLAUDE.md states "compliance dashboard (sole UI exception)"; a general adapter dashboard violates the air-gap UI rule | mai-api | DEFER + rationale | W8 |
| 15 | email + PERF-004 | "JSON.parse / stringify inside loops" | PERF-004 LOW | TRUE — `.integrity/mcp-server/server.js:317` does `JSON.stringify(..., null, 2)` per loop iteration | mcp-server | P3 | W6 |
| 16 | email + QUA-001 | "God files over 300 lines" | QUA-001 MEDIUM | TRUE — `server.js` is 371 lines; vllm `adapter.py` is 332; ollama is 316 (acceptable for full backend adapter) | mcp-server primarily | P3 | W6 |
| 17 | email + QUA-009 | "Deeply nested code (4+ levels)" | QUA-009 MEDIUM | TRUE — `server.js:69` flagged at 4 levels | mcp-server | P3 | W6 |
| 18 | email + TST-005 | "No integration or e2e tests" | TST-005 MEDIUM | Partial — Rust workspace ships 1539 tests + 6 compliance demos; Python adapter layer lacks live-backend integration; no claudewide e2e script | mai workspace | P2 | W3 |
| 19 | email + PRJ-002 | "Incomplete gitignore … node_modules, .env, dist, build" | PRJ-002 MEDIUM | Partial — `.env`, `.venv`, `dist`, `build` are present; `node_modules` is missing (needed because the MCP server uses npm) | packaging | P2 | W2 |
| 20 | email + PRJ-005 | "Flat project structure" | PRJ-005 (PASSED in scan) | FALSE — `mai/` already organises into crates (`mai-api`, `mai-compliance`, `mai-scheduler`, `mai-core`, …), `adapters/{ollama,llamacpp,…}/`, `docs/`, `tests/`, `.integrity/`. Scan flag did not fire; John mis-paraphrased | n/a | refute | W8 |

### 2.1 Other GitDoctor findings that didn't make it into the email but are in the screenshots

| ID | Description | Real? | Action |
|:--|:--|:--|:--|
| SEC-010 | Insecure cookie handling | scan PASSED (15/16) — applies to web-app contexts MAI doesn't have | none |
| SEC-011 | No rate limiting on API routes | mai-api ships an axum middleware; needs audit | W4 |
| SEC-012 | Schema validation on inputs | mai-api uses serde with deny_unknown_fields where critical; needs audit | W4 |
| SEC-013 | Debug mode in config | air-gap profile enforces production via `ProfileMode::Production`; safe | refute in W8 |
| SEC-014/015 | Mutation/upload routes unauthed | mai-api has auth middleware (PROD-AUTH-002 + PROD-AUTH-101); safe | refute in W8 |
| CFG-001 | Hardcoded localhost URLs | INTENTIONAL — air-gap design | refute in W8 |
| CFG-005 | Missing `.env.example` | Real but trivial | W2 |
| CFG-006 | TypeScript strict mode | MCP server has no `tsconfig.json`; it's plain JS | refute or convert in W6 |
| QUA-002 | Functions with too many parameters | unclear scope; audit | W6 |
| QUA-005 | Excessive console.log | mcp-server uses console.log; should use a logger | W6 |
| QUA-006 | Commented-out code blocks | audit | W6 |
| QUA-007 | Mixed `.then` and `async/await` | audit | W6 |
| QUA-008 | Modules with 15+ exports | audit | W6 |
| QUA-010 | `.then()` without `.catch()` | audit | W6 |
| TST-002 | Test-to-source ratio under 10% | mai-* ratio is healthier than scanner estimates; needs evidence in W8 | W8 |
| TST-003 | Empty test bodies | audit thin adapter tests | W5 |

---

## 3. Workstreams

Workstreams group sessions by deliverable theme, not by file. Sessions inside a workstream can sometimes run in parallel; workstream-to-workstream dependencies are explicit in §4.

### W1 — Security Critical (P0)
Replace `Math.random()` in the MCP server's staged-write tmp-path generator with `crypto.randomUUID()` or `crypto.randomBytes(16).toString('hex')`. This is one targeted Edit. No design discussion needed; the fix is mechanical. Verification: re-run GitDoctor and confirm SEC-009 disappears.

### W2 — Packaging & Lock Files (P1)
Three deliverables, one new lock-file regime, one Dockerfile, one `.gitignore` patch, one `.env.example`. Specifically:

1. Pin Python dependencies: add `requirements.txt` generated from `pyproject.toml` deps plus a `requirements-lock.txt` produced by `pip-compile` (uv-equivalent acceptable if uv is the preferred tool). Decision: pip-tools vs uv lockfile is left to J-03 to choose with a one-paragraph rationale.
2. Add `.integrity/mcp-server/package-lock.json` by running `npm install --package-lock-only` and committing the result.
3. Add a multi-stage Dockerfile at `mai/Dockerfile`: stage 1 builds `mai-api` with `cargo build --release` against a pinned `rust:1.95` base; stage 2 is a minimal distroless image with the binary + a Python venv layer for the adapter runner. Include `.dockerignore`. Document that GPU runtime is out-of-scope for the initial Dockerfile — CPU-only smoke target.
4. Patch `.gitignore` to add `node_modules/` (and `*.log` if not already covered).
5. Add `.env.example` with every env var the README-FIRST quickstart references (no real secrets).

### W3 — Adapter Completion Matrix (P1/P2)
Goal: bring every adapter surface to completion status. Ollama is the reference implementation, not the end of the lane. Every backend must either implement each `AdapterBase` method correctly or return a typed `UnsupportedOperationError` with tests proving the capability flag is honest. No silent stubs, no fake success, no "works in mocks only" claims.

**Completion gate for every adapter:**

1. Full method surface: `initialize`, `generate`, `stream`, `embed` where supported, `health`, `capabilities`, `shutdown`, and lifecycle cleanup.
2. Live-backend integration: opt-in tests marked `live_backend`, skipped cleanly when the backend is unavailable, passing against a real backend when env vars are supplied.
3. Connection pooling: long-lived HTTP clients or documented N/A path; no per-request client creation in hot paths.
4. Error mapping: backend unavailable, model missing, timeout, context overflow, malformed response, rate limit, and backend crash map to typed adapter errors.
5. Capability truthfulness: every `supports_*` flag is backed by at least one unit or live test.
6. Resource lifecycle: `async with` works after W7, shutdown is idempotent, no leaked sessions/subprocesses.
7. Assertions: each adapter test file has meaningful assertions, not smoke-only execution.

**Mandatory in-repo adapters (must close in this lane):**

1. **J-06: Ollama** — local developer/default path; live generate, stream, embeddings, health, model missing, backend down, lifecycle cleanup.
2. **J-07: llama.cpp** — GGUF/CPU/Metal/CUDA path; live `llama-server`, grammar/structured output, streaming, context overflow, shutdown.
3. **J-18: vLLM** — production GPU serving path; OpenAI-compatible chat/completions/stream/embeddings, batching, tool calling or structured output where supported, health/model lifecycle.
4. **J-19: TGI** — Hugging Face Text Generation Inference; streaming, batching behavior, unavailable backend, model readiness, error mapping.
5. **J-20: SGLang** — constrained/structured decoding path; streaming, structured output, tool calling, batching, backend errors.
6. **J-21: ExLlamaV2** — quantized single-node GPU path; generation, streaming, hot-swap/multi-model claims, unsupported embedding behavior, lifecycle cleanup.
7. **J-22: TensorRT-LLM/Triton** — NVIDIA production path; live Triton/TensorRT smoke, batching, model readiness, degraded hardware cases, timeout and crash mapping.

**Valuable additions for comprehensive market/runtime coverage:**

8. **J-23: Generic OpenAI-compatible local adapter** — covers LM Studio, LocalAI, FastChat-style servers, and custom internal gateways implementing `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, and `/v1/models`.
9. **J-24: ONNX Runtime adapter** — CPU/DirectML/enterprise Windows fallback path; useful for deterministic non-NVIDIA deployments.
10. **J-25: MLX adapter** — Apple Silicon local inference path; valuable for secure edge/dev nodes.
11. **J-26: Generic Triton adapter** — non-LLM Triton inference path distinct from TensorRT-LLM; prepares MAI for multimodal/classifier workloads.

**J-05 (audit & spec)** now produces `mai/docs/ADAPTER-COMPLETION-MATRIX.md`, not just a pooling note. It profiles every existing adapter for method completeness, live-test coverage, pooling, lifecycle cleanup, capability truthfulness, and missing error cases. It also defines the exact acceptance checklist for J-18..J-26.

Connection pooling fixes are folded into each backend's completion session if the J-05 audit shows a per-request pattern. W3 does not close until all 11 adapters above are either implemented and tested or, for hardware-gated paths, have opt-in live tests plus deterministic local mocks that prove the API contract.

### W4 — Error Handling Audit (P1)
Walk every critical path in `mai-api` (handler → adapter manager → adapter → backend) and confirm errors propagate via the typed taxonomy from `adapters/base.py::AdapterError` and the mai-api `ErrorResponse` shape. Fix gaps. Output: a one-page "Error Path Audit" doc in `mai/docs/ERROR-PATH-AUDIT.md` listing each path, status (PASS/FIX), and the fix commit. Adds rate-limit + schema-validation audits as sub-items (SEC-011, SEC-012).

### W5 — Test Quality Fill (P2)
Replace empty/thin adapter tests. Specifically:

1. `adapters/llamacpp/tests/test_adapter.py` — grow from 14 assertions to at least 30, covering: init failure modes, generation happy path with deterministic seed, streaming, embedding (if supported by backend), health, shutdown idempotency.
2. `adapters/exllamav2/tests/test_adapter.py` — same shape, 13 → 30 assertions.
3. Audit `adapters/*/tests/__init__.py` — if empty, leave alone (package markers).
4. Add an "assertion count" CI check: a single `pytest` plugin or grep-based gate that fails if any `test_*.py` has fewer than 3 assertions. Codifies the rule.

### W6 — Node MCP Server Code Hygiene (P3)
`.integrity/mcp-server/server.js` is the focal point of half the GitDoctor flags. One session refactors it:

1. Split into `server.js` (bootstrap), `handlers.js` (per-tool functions), and `validators.js` (per-check shared code). Each under 200 lines.
2. Lift the `JSON.stringify(..., null, 2)` call at line 317 out of its loop.
3. Replace `console.log` with a minimal `pino`/`bunyan` logger pinned in `package.json`.
4. Reduce the 4-level nesting at line 69 by extracting helpers and early returns.
5. Add `tsconfig.json` with `strict: true` and convert to TypeScript ONLY if the cost is < 1 hour. If conversion balloons, leave as JS but add `// @ts-check` and JSDoc types — that's enough to clear CFG-006.

### W7 — Adapter Lifecycle + Health Aggregator (P2/P3)
1. Add `__aenter__` / `__aexit__` to `AdapterBase` that call `initialize` and `shutdown` respectively. All concrete adapters inherit. Test that `async with OllamaAdapter() as a: ...` cleans up.
2. Add a `/health/system` aggregator endpoint in `mai-api` that fans out to every registered adapter's `health()` and returns a single JSON document with per-backend status plus an overall rollup. No new dependencies.

### W8 — Refutation Evidence Pack (P0)
The single highest-value deliverable. A `mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md` that walks point-by-point through John's email AND every GitDoctor finding, with evidence:

- Adapter-stub claim: paste of `wc -l` and `grep -c 'NotImplementedError' adapters/*/adapter.py` showing the implementations.
- Stdlib-only critique: pointer to ARCHITECTURE.md and the threat-model section explaining why third-party deps are an air-gap regression, plus a one-paragraph statement of the policy.
- Flat-structure claim: tree output showing the actual directory layout.
- Dashboard-deferral: cite CLAUDE.md UI rule + propose alternative (CLI status command).
- All real findings: cite the J-session that fixes each, with commit hash to be filled in as sessions close.

This doc is the package John receives in response. It is the human-facing artifact of the entire lane.

### W10 — mai-sdk-rs HTTP Client + Streaming (P1)
The Rust SDK was scaffolded in S11 with the entire HTTP client surface marked `todo!("Session 11: HTTP client")` — 17 sites in total at `mai/mai-sdk-rs/src/lib.rs` lines 768-887. Closing this workstream requires two sessions:

1. **J-16: SDK HTTP client.** Implement the 14 plain-HTTP method bodies (everything that is not SSE streaming or resume). Add `reqwest` (or `hyper` direct, if Basho prefers no transitive TLS dep) as a workspace-pinned dependency. Add a `tests/` directory with unit tests against `wiremock` for happy-path + auth + retry + error-mapping. Goal: every `todo!("HTTP client")` replaced with a real body; `cargo doc` shows a usable client API; downstream consumers (the application scaffolds in S30-S31) compile against the real surface.
2. **J-17: SDK SSE streaming + resume protocol.** Implement the 3 streaming `todo!()` sites (`stream`, `stream_resume`, `resume`). Add `eventsource-client` (or implement a minimal SSE parser if the crate footprint is unwelcome). Add an integration test that runs against a local mai-api spun up in the test harness, exercises a streamed inference, kills the connection mid-stream, and resumes from the last event id.

Split rationale: 999 LOC file, 17 stub sites, plus new transitive deps, plus new test directory exceeds the anti-truncation blast-radius safe in a single session. Splitting on the HTTP/SSE seam mirrors how S11 originally carved the surface.

### W9 — Re-scan + Close (P1)
1. **J-14:** Re-run GitDoctor against the post-W1..W7 + W10 HEAD, including all adapter completion sessions J-18..J-26. Capture all screenshots into `mai/test-evidence/dougherty-rescan/`. Target: overall ≥75/100, Code Quality ≥70, Testing ≥60, zero HIGH security findings, zero HIGH project findings.
2. **J-15:** Bundle the response. Update `RC1-TESTER-FEEDBACK.md` with the John section closed. Email John (drafted in `mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md` for Basho to send) with: response doc, updated bundle SHA-256, re-scan screenshots, list of items deferred-with-rationale.

---

## 4. Session sequence & dependency graph

```
  J-01 ──┐                                  W1 Security
         │
  J-08 ──┤                                  W4 Error audit (parallelisable)
         │
  J-02 ─ J-03 ─ J-04                        W2 Packaging (sequential)
         │
  J-05 ─ J-06 ─ J-07                       W3 Adapter completion matrix starts
         │   │
         └─ J-18 ─ J-19 ─ J-20 ─ J-21 ─ J-22 Mandatory remaining adapters
              │
              └─ J-23 ─ J-24 ─ J-25 ─ J-26 Valuable adapter additions
         │
  J-09 ─ J-10                               W5 Test quality (sequential)
         │
  J-11                                      W6 MCP server cleanup (independent)
         │
  J-12 ─ J-13                               W7 Lifecycle + health (parallelisable)
         │
  J-16 ─ J-17                               W10 SDK HTTP then SSE+resume (parallelisable with W3/W5/W6/W7)
         │
  J-XX  ────── (W8 Evidence pack: J-XX — written EARLY, refreshed at end)
         │
         v
  J-14  Rescan  (depends on J-01..J-13 AND J-16..J-26)
         │
         v
  J-15  Send + RC-10 prep
```

**Critical path (longest sequential chain):** J-02 → J-03 → J-04 → J-05 → J-06 → J-07 → J-18 → J-19 → J-20 → J-21 → J-22 → J-23 → J-24 → J-25 → J-26 → J-16 → J-17 → J-14 → J-15. Roughly 19 sessions of serial work, with safe parallelism possible across independent adapter sessions once J-05 defines the shared matrix.

**Recommended order to start:** J-01 (one Edit, ten minutes, kills the HIGH security flag and is the cheapest morale win); then J-08-evidence-pack-draft (refute false positives publicly before doing any other work — protects Basho's standing with John); then resume sequential.

---

## 5. Acceptance criteria per workstream

| Workstream | Acceptance |
|:--|:--|
| W1 | GitDoctor re-scan shows zero SEC-009 hit. `git grep 'Math.random'` returns zero hits inside `.integrity/`. |
| W2 | `requirements-lock.txt` (or `uv.lock`) present and CI verifies it parses. `package-lock.json` present in `.integrity/mcp-server/`. `mai/Dockerfile` builds end-to-end (CPU-only) on a fresh Linux VM in CI, produces a runnable image that responds to `/health`. `.gitignore` includes `node_modules/`. `.env.example` covers every README-FIRST-referenced var. |
| W3 | `mai/docs/ADAPTER-COMPLETION-MATRIX.md` exists and covers Ollama, llama.cpp, vLLM, TGI, SGLang, ExLlamaV2, TensorRT-LLM/Triton, generic OpenAI-compatible, ONNX Runtime, MLX, and generic Triton. Every adapter has unit tests, opt-in live-backend tests, pooling/lifecycle coverage, typed error mapping, honest capability flags, and completion status recorded. `pytest -m live_backend` passes for every backend whose env vars are supplied and skips cleanly otherwise. |
| W4 | Audit doc covers every API handler. Every handler that reaches an adapter has a documented error-mapping path. SEC-011 (rate limit) and SEC-012 (schema validation) cleared in re-scan. |
| W5 | Every `test_adapter.py` has ≥30 assertions. CI gate fails when any new `test_*.py` has fewer than 3 assertions. GitDoctor TST-004 cleared in re-scan. |
| W6 | `server.js` ≤200 lines. QUA-001, QUA-009, PERF-004, QUA-005 cleared in re-scan. `package.json` declares the logger dep. |
| W7 | `async with` works on every adapter. `/health/system` endpoint exists, has a Rust integration test against the in-process adapter set. |
| W8 | Response doc lands in `mai/docs/`, references every John item with a verdict + evidence + (if applicable) J-session that fixes it. The SDK section honestly states stubs existed at the scan time and points at J-16/J-17 commits — no refutation framing on this row. |
| W10 | `grep -rn "todo!" mai/mai-sdk-rs/src/` returns zero hits. `cargo test -p mai-sdk-rs` passes (new `tests/` directory exists). `cargo doc -p mai-sdk-rs --no-deps` shows method bodies for every public client method. Cargo.toml carries the new transport dep pinned to a specific version. SHIP `KNOWN-ISSUES.md` Issue 15 marked CLOSED with the J-16/J-17 commit hashes. |
| W9 | Re-scan screenshots stored. Overall ≥75. Email-ready response file ready for Basho to send. |

---

## 6. Definition of Done for the DOUGHERTY lane

The lane closes when ALL of:

1. Every J-session in the roster is committed with the canonical co-author footer.
2. `mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md` is committed and references every J-session by commit hash.
3. GitDoctor re-scan attached as PNGs in `mai/test-evidence/dougherty-rescan/`, with the score deltas summarised in §3 of the response doc.
4. The RC1 v2 bundle at `C:/Users/17076/Documents/Claude/Island-Mountain-RC1-release/Lamprey-MAI-RC1/` is rebuilt to include the post-DOUGHERTY commits, with new SHA-256s in `CHECKSUMS.txt`.
5. `RC1-TESTER-FEEDBACK.md §9` is updated: John's section moved from "open" to "responded — awaiting tester re-test".
6. RC-10 prerequisites declared met in a one-line update to `project_rc_release_lane` memory.

The lane does NOT close on:
- John replying. The lane closes when the response ships. John's re-test is the RC-10 input.
- 100/100 GitDoctor score. The target is ≥75 with zero HIGH-severity findings — pursuing 100 would create vibe-coded scope drift.

---

## 7. Risks and mitigations

| Risk | Mitigation |
|:--|:--|
| Live-backend integration tests are flaky on CI runners without GPU | Mark `@pytest.mark.live_backend`, gate on env var, run in a separate job that's allowed to fail without blocking main pipeline. Document in CONTRIBUTING. |
| Dockerfile balloons (CUDA, multi-arch, etc.) | Scope J-04 to CPU-only, single-arch (x86_64-linux). GPU containerisation is a separate future session, explicitly noted in the Dockerfile header. |
| Refactoring `server.js` corrupts the integrity tooling | All edits go through the staged-write protocol. After refactor, run the workspace's own `quick-check.sh` against three sample files end-to-end to prove the tooling still functions. |
| Evidence pack feels defensive | Lead with the real fixes, not the refutations. Structure: §1 What we fixed, §2 What we measured, §3 What we deferred and why, §4 What we believe the scan got wrong (with evidence). |
| Sandbox disk fills mid-session | Per workspace CLAUDE.md §"Sandbox Disk Space Rules": every session checks `df -h /` before any cargo/pip/npm. J-04 specifically must purge cargo target before image build. |
| Anti-truncation rules violated | Every session ≥3 files spawns the verification subagent before commit. Every new file >40 lines uses staged-write. Hard gate, no exceptions. |

---

## 8. Out of scope for this lane (and why)

- Building a web dashboard (John item 14): conflicts with the CLAUDE.md "compliance dashboard is sole UI exception" rule. Documented in W8 with proposed CLI alternative.
- Replacing stdlib with third-party crates (John item 3): contradicts the air-gap threat model for the **inference + compliance core**. Documented in W8. Note: this carve-out does NOT apply to `mai-sdk-rs`, which is consumed by L4-L5 application scaffolds OUTSIDE the air-gap boundary — pulling `reqwest`/`eventsource-client` into the SDK in J-16/J-17 is consistent with the architecture, not in tension with it.
- Restructuring the project layout (John item 20): the layout is already organised; the scan flag did not fire, John mis-paraphrased.
- Pursuing 100/100 GitDoctor score: scope drift, no production value.
- Containerising GPU runtime: separate future session; J-04 is CPU-only.
- Migrating MCP server from JS to a different language: J-11 may convert to TS if cheap, otherwise stays JS-with-jsdoc-types.

---

## 9. Companion document

The per-session prompts are in `JOHN-REMEDIATION-ROSTER.md` (same directory). That document is self-contained: every J-session prompt is briefed for an agent walking in cold, with file paths, expected line deltas, verification steps, and the co-author footer reminder.
