# John Dougherty Remediation Prompt Roster

> **STATUS — CLOSED (2026-05-25). SUPERSEDED.**
> Original draft. Canonical re-emit lives at [`../JOHN-REMEDIATION-ROSTER.md`](../JOHN-REMEDIATION-ROSTER.md). Lane closed 2026-05-25 with all 27 sessions complete (top-level roster adds J-10b). See [`J-15-DOUGHERTY-CLOSURE.md`](J-15-DOUGHERTY-CLOSURE.md).

**Lane:** DOUGHERTY (sessions `J-01` through `J-26`; amended 2026-05-24 to add SDK work in W10 and comprehensive adapter completion in W3)
**Companion plan:** `JOHN-REMEDIATION-PLAN.md` (read it first)
**Source feedback:** John Dougherty email 2026-05-24 + GitDoctor scan screenshots in `.tester-feedback-2026-05-24/`
**Authoring convention:** every prompt below is self-contained for an agent walking in cold with no conversation history. Each carries file paths, expected line deltas, verification steps, and an explicit reminder of the canonical co-author footer and the staged-write protocol.
**Commit footer (every commit, no exceptions):**
```
Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```
**Anti-truncation gate:** every session that writes ≥3 files must spawn a verification subagent before commit per workspace `CLAUDE.md`. Every new file >40 lines uses the two-stage staged-write protocol.

---

## Session index

| ID  | Title                                                       | Workstream | Files (approx) | Depends on | Effort |
|:----|:------------------------------------------------------------|:-----------|:---------------|:-----------|:-------|
| J-01| Replace Math.random in MCP server with crypto.randomUUID    | W1         | 1              | —          | XS     |
| J-02| Add node_modules + log gitignore entries                    | W2         | 1              | —          | XS     |
| J-03| Python + Node dependency lock files                         | W2         | 3-4            | J-02       | S      |
| J-04| CPU-only multi-stage Dockerfile + .dockerignore + .env.ex   | W2         | 3              | J-03       | M      |
| J-05| Adapter completion matrix + pooling audit                   | W3         | 1 (doc)        | —          | M      |
| J-06| Ollama live-backend integration test suite                  | W3         | 2-3            | J-05       | M      |
| J-07| llama.cpp live-backend integration test suite               | W3         | 2-3            | J-06       | M      |
| J-18| vLLM adapter completion                                     | W3         | 2-4            | J-05       | M      |
| J-19| TGI adapter completion                                      | W3         | 2-4            | J-05       | M      |
| J-20| SGLang adapter completion                                   | W3         | 2-4            | J-05       | M      |
| J-21| ExLlamaV2 adapter completion                                | W3         | 2-4            | J-05       | M      |
| J-22| TensorRT-LLM/Triton adapter completion                      | W3         | 2-4            | J-05       | M-L    |
| J-23| Generic OpenAI-compatible local adapter                     | W3         | 4-6            | J-05       | M      |
| J-24| ONNX Runtime adapter                                        | W3         | 4-6            | J-23       | M-L    |
| J-25| MLX adapter                                                 | W3         | 4-6            | J-23       | M-L    |
| J-26| Generic Triton adapter                                      | W3         | 4-6            | J-22       | M-L    |
| J-08| Error path audit + rate-limit + schema-validation checks    | W4         | 1 (doc) + N    | —          | M      |
| J-09| Adapter test assertion fill (llamacpp + exllamav2)          | W5         | 2              | —          | M      |
| J-10| Pytest assertion-count gate + e2e compliance smoke          | W5         | 2-3            | J-09       | S      |
| J-11| Refactor MCP server.js into focused modules                 | W6         | 4-5            | J-01       | M      |
| J-12| Async context managers on AdapterBase + concrete adapters   | W7         | 8 (small edits)| —          | S      |
| J-13| /health/system aggregator endpoint                          | W7         | 2              | J-12       | S      |
| J-16| mai-sdk-rs HTTP client implementation                       | W10        | 3-4            | —          | M-L    |
| J-17| mai-sdk-rs SSE streaming + resume protocol                  | W10        | 2-3            | J-16       | M      |
| J-14| Re-run GitDoctor, capture evidence                          | W9         | evidence only  | J-01..J-13, J-16..J-26 | XS |
| J-15| Draft response doc + RC-10 prep (evidence pack lands here)  | W8, W9     | 2              | J-14       | M      |

**XS** ≤30 min, **S** ≤1 hr, **M** ≤2 hr, **L** ≤4 hr.

---

### Session J-01: Replace Math.random in MCP server with crypto.randomUUID

**Workstream:** W1 (Security Critical)
**Depends on:** none
**Blocks:** J-11 (which refactors the same file)
**Files in play:** `mai/.integrity/mcp-server/server.js` (1 file, single-line Edit)
**Expected line delta:** ±0 to +1 (one require + one expression change)

#### Context Brief

GitDoctor flagged `SEC-009 HIGH: Math.random() in security-sensitive context` at `mai/.integrity/mcp-server/server.js:233`. The call is `${tmpdir()}/mai-safe-write-${Date.now()}-${Math.random().toString(36).slice(2)}`, generating a tmp path for the staged-write tool. `Math.random` is not cryptographically secure and is predictable across processes. Use `node:crypto`.

#### Prompt

SESSION J-01: Replace Math.random in MCP server

CONTEXT: GitDoctor flagged Math.random in security-sensitive context at
mai/.integrity/mcp-server/server.js line 233. Replace with crypto-strong randomness.

IMPLEMENT:
1. At the top of mai/.integrity/mcp-server/server.js, add: `const { randomUUID } = require("node:crypto");` (or import equivalent depending on the module style already in the file — check the existing requires).
2. Replace the expression `${Date.now()}-${Math.random().toString(36).slice(2)}` with `${Date.now()}-${randomUUID()}`.
3. No other changes.

VERIFY:
- `node -e "require('./mai/.integrity/mcp-server/server.js')"` does not throw (or however the server is currently launched in the smoke check).
- `grep -n "Math.random" mai/.integrity/mcp-server/server.js` returns nothing.
- File line count is within ±1 of the original (371 lines).

COMMIT: one commit, message `J-01: replace Math.random with crypto.randomUUID in MCP server`. Footer:
```
Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
```

#### Acceptance Criteria

- `grep -c "Math.random" mai/.integrity/mcp-server/server.js` returns `0`.
- Smoke launch of the MCP server (`node mai/.integrity/mcp-server/server.js` if standalone, or via the existing harness) returns no error.
- File still passes `node --check`.
- Commit footer present.

---

### Session J-02: Patch .gitignore for node_modules and logs

**Workstream:** W2 (Packaging)
**Depends on:** none
**Blocks:** J-03 (lock files land into a now-clean tree)
**Files in play:** `mai/.gitignore` (1 file, append-only)
**Expected line delta:** +3 to +5

#### Context Brief

GitDoctor flagged `PRJ-002 MEDIUM: Incomplete .gitignore`. The current file (`mai/.gitignore`, 38 lines) covers Rust target/, Python caches, OS metadata, .env, .venv, dist, build, test artifacts. It does NOT cover `node_modules/` even though the MCP server uses npm. Add it. Also add `*.log` if not present.

#### Prompt

SESSION J-02: Patch mai/.gitignore for node_modules and logs

CONTEXT: GitDoctor flagged missing node_modules and log entries in mai/.gitignore.

IMPLEMENT:
1. Append a new section to mai/.gitignore:
```
# Node (MCP server)
node_modules/
package-lock.json.bak
*.log
```
   Skip any line that already exists in the file. Use Edit tool, not Write.

VERIFY:
- `git check-ignore -v mai/.integrity/mcp-server/node_modules/anything 2>/dev/null` succeeds (returns the rule).
- File length is original + 4 to original + 5.

COMMIT: one commit, message `J-02: gitignore node_modules and log files`. Footer per global rule.

#### Acceptance Criteria

- `git check-ignore` reports `node_modules/` as ignored.
- `*.log` is ignored.
- No accidental deletion of existing lines.

---

### Session J-03: Python and Node dependency lock files

**Workstream:** W2 (Packaging)
**Depends on:** J-02
**Blocks:** J-04 (Dockerfile uses the lock files)
**Files in play:** `mai/requirements.txt`, `mai/requirements-lock.txt`, `mai/.integrity/mcp-server/package-lock.json`, optionally `mai/pyproject.toml`
**Expected line delta:** new lock files (~hundreds of lines each, machine-generated)

#### Context Brief

GitDoctor flagged `PRJ-004 HIGH: Missing dependency lock file`. `Cargo.lock` exists. Python uses `pyproject.toml` with no lock. The MCP server has `package.json` with no `package-lock.json`. Add both.

For Python: choose pip-tools (`pip-compile`) OR uv (`uv lock`). Decision is whichever Basho already has installed; default to pip-tools because it produces a portable `requirements-lock.txt` that doesn't require uv on the consumer side.

#### Prompt

SESSION J-03: Add Python and Node dependency lock files

CONTEXT: PRJ-004 HIGH: missing dependency lock files. Cargo.lock exists; need Python and Node equivalents.

DECISION POINT: pick pip-tools vs uv for Python locking. Default = pip-tools (produces requirements-lock.txt that works without uv on the consumer side). Document the choice in mai/docs/LOCK-FILE-POLICY.md (new file, ~30 lines).

IMPLEMENT:
1. Generate mai/requirements.txt from the [project.dependencies] section of mai/pyproject.toml. If pyproject.toml has no concrete deps listed, derive them from imports in adapters/*/adapter.py and adapters/base.py. The file should pin top-level deps with == versions matching what the current Python environment has installed.
2. `pip-compile --generate-hashes mai/requirements.txt -o mai/requirements-lock.txt`. If pip-compile is unavailable, fall back to `pip freeze > mai/requirements-lock.txt`.
3. In mai/.integrity/mcp-server/, run `npm install --package-lock-only` (this creates package-lock.json without installing node_modules). Commit the resulting package-lock.json.
4. Write mai/docs/LOCK-FILE-POLICY.md (new file, MUST go through staged-write protocol since >40 lines). Cover: tooling chosen, regeneration command, when to update, how the Dockerfile and CI reference these files.

VERIFY:
- `python -m pip install --dry-run -r mai/requirements-lock.txt` resolves cleanly.
- `cd mai/.integrity/mcp-server && npm ci --dry-run` resolves cleanly.
- LOCK-FILE-POLICY.md tail matches intent (last line is a complete sentence).

COMMIT: separate commits per artifact OR one bundled commit with message `J-03: pin Python deps (requirements-lock.txt) and Node deps (package-lock.json), document policy`. Subagent verification required (3+ files). Footer per global rule.

#### Acceptance Criteria

- `mai/requirements.txt` and `mai/requirements-lock.txt` both present.
- `mai/.integrity/mcp-server/package-lock.json` present, valid JSON.
- `mai/docs/LOCK-FILE-POLICY.md` present, ≥30 lines, references both lock files and the regeneration commands.
- GitDoctor re-scan would clear PRJ-004.

---

### Session J-04: CPU-only multi-stage Dockerfile

**Workstream:** W2 (Packaging)
**Depends on:** J-03 (Dockerfile uses lock files)
**Blocks:** J-14 (re-scan should clear CFG-007)
**Files in play:** `mai/Dockerfile`, `mai/.dockerignore`, `mai/.env.example`
**Expected line delta:** Dockerfile ~80 lines, .dockerignore ~20 lines, .env.example ~30 lines

#### Context Brief

GitDoctor flagged `CFG-007 LOW: No Dockerfile for containerization`. Also `CFG-005: Missing .env.example`. Add both. Scope: CPU-only, x86_64-linux, distroless final layer. GPU containerisation is explicitly out of scope (separate future session).

The base image MUST be pinned to a digest, not a tag (PRJ-004-adjacent: reproducible builds). Use `rust:1.95-slim@sha256:...` style pins.

#### Prompt

SESSION J-04: Multi-stage CPU-only Dockerfile

CONTEXT: CFG-007 missing Dockerfile, CFG-005 missing .env.example. Scope: CPU-only, x86_64-linux, distroless final.

IMPLEMENT:
1. mai/Dockerfile (new, staged-write protocol; ~80 lines):
   - Stage 1 `rust-builder`: pinned `rust:1.95-slim-bookworm` base. Copy Cargo.toml, Cargo.lock, then crates/ source. Run `cargo build --release -p mai-api -p mai-ship-validate`. Cache mount on /usr/local/cargo/registry.
   - Stage 2 `python-builder`: pinned `python:3.12-slim` base. Copy requirements-lock.txt. `pip install --no-deps -r requirements-lock.txt --target=/python-deps`.
   - Stage 3 `runtime`: distroless `gcr.io/distroless/cc-debian12:nonroot`. Copy mai-api and mai-ship-validate binaries from stage 1. Copy /python-deps from stage 2. Copy mai/adapters/ source. EXPOSE 8420. ENTRYPOINT mai-api.
   - Document the stage pins, the lack of GPU support, and the regeneration command in a header comment block.

2. mai/.dockerignore (new, staged-write since likely >40 lines after exclusions):
   - target/, .git/, .venv/, node_modules/, *.log, test-evidence/, docs/, *.md (except README.md), .integrity/, tests/burn-in/, tests/benchmarks/

3. mai/.env.example (new, staged-write):
   - Document every env var the README-FIRST references: MAI_PROFILE, MAI_API_BIND, MAI_AUTH_KEYS_PATH, MAI_AUDIT_LOG_PATH, MAI_VAULT_TOKEN (with a clear "REPLACE-ME" sentinel), MAI_LOG_LEVEL, RUST_LOG, OLLAMA_HOST, LLAMACPP_HOST.

VERIFY (in a fresh shell):
- `docker build -t mai-rc1:dockerfile-test mai/` succeeds (network permitting; if no docker available, leave Dockerfile in place and document the manual verification command in the commit message).
- The resulting image responds to `curl http://localhost:8420/health` after `docker run -p 8420:8420 mai-rc1:dockerfile-test`.
- Subagent verification required (3 new files).

COMMIT: one commit, message `J-04: CPU-only multi-stage Dockerfile + .dockerignore + .env.example`. Footer per global rule.

#### Acceptance Criteria

- `mai/Dockerfile` builds end-to-end CPU-only.
- Image starts and answers /health.
- `mai/.env.example` covers every README-FIRST env var.
- Header comment in Dockerfile documents GPU out-of-scope.
- GitDoctor re-scan clears CFG-007 and CFG-005.

---

### Session J-05: Adapter completion matrix + pooling audit

**Workstream:** W3
**Depends on:** none
**Blocks:** J-06, J-07, J-18..J-26
**Files in play:** `mai/docs/ADAPTER-COMPLETION-MATRIX.md` (new, ~180 lines)
**Expected line delta:** +180 (doc only)

#### Context Brief

John's email and GitDoctor tip "Implement Connection Pooling MEDIUM" are too narrow if treated as Ollama-only. This session establishes the adapter completion standard for all mandatory and expansion adapters. It audits method completeness, live-test coverage, pooling, lifecycle cleanup, capability truthfulness, and error mapping before implementation sessions begin.

#### Prompt

SESSION J-05: Build adapter completion matrix and audit pooling

CONTEXT: Need evidence before fixing. Audit-only session. W3 covers all current adapters plus four valuable additions; Ollama is only the reference implementation.

IMPLEMENT:
1. Read every existing adapter under `mai/adapters/`: ollama, llamacpp, vllm, tgi, sglang, exllamav2, tensorrt.
2. Create `mai/docs/ADAPTER-COMPLETION-MATRIX.md` (new, staged-write). Include one section per mandatory adapter and one section per expansion adapter:
   - Mandatory: Ollama, llama.cpp, vLLM, TGI, SGLang, ExLlamaV2, TensorRT-LLM/Triton.
   - Additions: generic OpenAI-compatible local adapter, ONNX Runtime, MLX, generic Triton.
3. For each existing adapter, document:
   - files read (`adapter.py`, `client.py`, `config.py`, tests)
   - method surface status: `initialize`, `generate`, `stream`, `embed`, `health`, `capabilities`, `shutdown`
   - client pattern: POOLED | PER-REQUEST | N/A, with exact file/line evidence
   - lifecycle status: manual only | async context ready | subprocess cleanup | HTTP session cleanup
   - capability truthfulness: each `supports_*` flag and which test proves it
   - error mapping gaps: backend unavailable, model missing, timeout, context exceeded, malformed response, rate limit, crash
   - live-test status: none | mocked only | opt-in live present | live passing
   - completion verdict: COMPLETE | NEEDS-FIX | HARDWARE-GATED | NEW-ADAPTER-NEEDED
4. For each expansion adapter, define:
   - why it exists
   - minimum public API
   - expected config schema and env var
   - unit-test matrix
   - live-test opt-in command
   - acceptance criteria for its implementation session
5. End the doc with a summary table:
   | Adapter | Category | Completion Verdict | Pooling | Live Test | Fix Session |
   The Fix Session column must map to J-06, J-07, or J-18..J-26.

VERIFY:
- Doc tail matches intent.
- Every existing backend in mai/adapters/ has a section.
- The four expansion adapters each have a section.
- Summary table references J-06, J-07, and J-18..J-26.

COMMIT: one commit, message `J-05: adapter completion matrix and pooling audit`. Footer per global rule.

#### Acceptance Criteria

- `mai/docs/ADAPTER-COMPLETION-MATRIX.md` exists with all 11 adapters.
- Each existing adapter has method, pooling, lifecycle, error, capability, and live-test verdicts.
- Each expansion adapter has a scoped implementation contract.
- Summary table cross-references J-06, J-07, and J-18..J-26.

---

### Session J-06: Ollama live-backend integration test

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-07
**Files in play:** `mai/adapters/ollama/tests/test_integration_live.py` (new), `mai/conftest.py` (edit), optionally `mai/adapters/ollama/client.py` (if J-05 audit said pooling fix needed)
**Expected line delta:** test file ~120 lines, conftest +20

#### Context Brief

John's email: "Replace the mocked adapter tests with actual integration tests that start real backends (Ollama, llama.cpp) and test the full request/response cycle." This session does Ollama. Skip-by-default; opt-in via env var.

#### Prompt

SESSION J-06: Ollama live-backend integration test

CONTEXT: Add real-backend integration tests for the Ollama adapter. Skip when Ollama is not available. Opt-in via env var.

IMPLEMENT:
1. mai/conftest.py: add a pytest mark `live_backend` registered via pytest_configure. Add a fixture `ollama_available` that returns True iff `os.environ.get("OLLAMA_HOST")` is set AND a `GET /api/tags` returns 200 within 2s. Skip live tests when the fixture is False.

2. mai/adapters/ollama/tests/test_integration_live.py (new, staged-write):
   - All tests marked `@pytest.mark.live_backend`.
   - Use the `ollama_available` fixture as autouse for the module — skip cleanly when no backend.
   - Tests (each must have ≥3 assertions):
     - `test_initialize_against_real_server`: initialize the adapter pointing at OLLAMA_HOST, verify capabilities() reflects real model list.
     - `test_generate_deterministic`: pull a tiny model (e.g. `qwen2:0.5b` if not present), generate with `temperature=0`, assert deterministic output AND assert FinishReason.STOP AND assert token count > 0.
     - `test_stream`: stream a 20-token request, assert events arrive incrementally (each chunk callback called >1 time), assert assembled text equals non-streamed equivalent.
     - `test_embeddings`: if backend supports it, embed two strings, assert vectors have equal length and that cosine similarity for identical strings is 1.0 within float tolerance.
     - `test_health`: call health(), assert status == "ok" AND latency_ms < 1000 AND uptime_s >= 0.
     - `test_shutdown_idempotent`: call shutdown twice, no exception, second call is no-op.

3. If J-05 audit said Ollama client needs pool fix: also patch mai/adapters/ollama/client.py to reuse a single `httpx.AsyncClient`. Tests above will catch regressions.

VERIFY:
- `OLLAMA_HOST=http://127.0.0.1:11434 pytest mai/adapters/ollama/tests/test_integration_live.py -v` passes against a real Ollama.
- Without OLLAMA_HOST set, the tests are skipped cleanly (not errored).
- Subagent verification required (2-3 new/edited files).

COMMIT: one commit, message `J-06: Ollama live-backend integration tests`. Footer per global rule.

#### Acceptance Criteria

- Tests exist with `@pytest.mark.live_backend`.
- Tests skip cleanly when Ollama unavailable.
- Tests pass against a real Ollama (Basho runs locally before commit).
- Pool fix applied if J-05 said so.

---

### Session J-07: llama.cpp live-backend integration test

**Workstream:** W3
**Depends on:** J-06 (mirror the fixture/mark pattern)
**Blocks:** —
**Files in play:** `mai/adapters/llamacpp/tests/test_integration_live.py` (new), edits to `conftest.py`, optionally a fixture helper `tests/_fixtures/llamacpp_model.py`
**Expected line delta:** test file ~120 lines

#### Context Brief

Same shape as J-06 but for llama.cpp's `llama-server`. Skip-by-default. Opt-in via `LLAMACPP_HOST` env var pointing at a running `llama-server`. Fixture may download a small GGUF on first run (under `tests/_models/`).

#### Prompt

SESSION J-07: llama.cpp live-backend integration test

CONTEXT: Same shape as J-06 but llama.cpp. Opt-in via LLAMACPP_HOST.

IMPLEMENT:
1. mai/conftest.py: add `llamacpp_available` fixture parallel to `ollama_available`. Use `GET /health` on the llama-server OpenAI-compat endpoint.

2. mai/adapters/llamacpp/tests/test_integration_live.py (new, staged-write):
   - All tests `@pytest.mark.live_backend`.
   - Skip via fixture.
   - Tests: initialize, generate deterministic (seed=42), stream, health, shutdown idempotent. Embeddings skipped if llama-server build does not support them (capabilities() will say so — assert via `pytest.skip` inside the test if cap is False).
   - Each test ≥3 assertions.

3. Optional: tests/_fixtures/llamacpp_model.py — helper to ensure a tiny GGUF (~50MB) exists at a known path. NOT downloaded by tests; tests fail with a clear message telling Basho to run `python tests/_fixtures/llamacpp_model.py download` first. This avoids surprise downloads in CI.

VERIFY:
- `LLAMACPP_HOST=http://127.0.0.1:8081 pytest mai/adapters/llamacpp/tests/test_integration_live.py -v` passes.
- Without LLAMACPP_HOST, tests skipped cleanly.

COMMIT: one commit, message `J-07: llama.cpp live-backend integration tests`. Footer per global rule.

#### Acceptance Criteria

- Tests exist, skip cleanly when unavailable, pass against a real llama-server.
- Fixture helper does not perform surprise downloads.

---

### Session J-18: vLLM adapter completion

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-14
**Files in play:** `mai/adapters/vllm/adapter.py`, `mai/adapters/vllm/client.py`, `mai/adapters/vllm/tests/test_adapter.py`, new `mai/adapters/vllm/tests/test_integration_live.py`
**Expected line delta:** test-focused unless J-05 identifies implementation gaps

#### Prompt

SESSION J-18: vLLM adapter completion

CONTEXT: Bring the vLLM adapter to the W3 completion gate. Use `ADAPTER-COMPLETION-MATRIX.md` as the source of truth for gaps.

IMPLEMENT:
1. Close every vLLM gap recorded by J-05: method completeness, pooling, lifecycle, typed errors, and capability truthfulness.
2. Add or expand unit tests for chat/completions, streaming, embeddings, batching, structured output/tool-calling if supported by the adapter, health, malformed response, model missing, backend down, timeout, and shutdown idempotency.
3. Add opt-in live tests marked `live_backend`, gated by `VLLM_HOST`, against a real vLLM OpenAI-compatible server.
4. Update `ADAPTER-COMPLETION-MATRIX.md` vLLM row to COMPLETE or HARDWARE-GATED with exact evidence.

VERIFY:
- `pytest mai/adapters/vllm/tests -v` passes.
- `VLLM_HOST=http://127.0.0.1:8000 pytest mai/adapters/vllm/tests/test_integration_live.py -v` passes when vLLM is running and skips cleanly otherwise.

COMMIT: one commit, message `J-18: vLLM adapter completion`. Footer per global rule.

---

### Session J-19: TGI adapter completion

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-14
**Files in play:** `mai/adapters/tgi/adapter.py`, `mai/adapters/tgi/client.py`, `mai/adapters/tgi/tests/test_adapter.py`, new `mai/adapters/tgi/tests/test_integration_live.py`

#### Prompt

SESSION J-19: TGI adapter completion

CONTEXT: Bring Hugging Face Text Generation Inference support to the W3 completion gate.

IMPLEMENT:
1. Close every TGI gap recorded by J-05.
2. Prove generation, streaming, batching behavior, health/model readiness, unavailable backend, timeout, malformed response, model missing, unsupported embeddings, and shutdown cleanup.
3. Add opt-in live tests gated by `TGI_HOST` against a real TGI server.
4. Update `ADAPTER-COMPLETION-MATRIX.md` TGI row with evidence.

VERIFY:
- `pytest mai/adapters/tgi/tests -v` passes.
- `TGI_HOST=http://127.0.0.1:8080 pytest mai/adapters/tgi/tests/test_integration_live.py -v` passes when TGI is running and skips cleanly otherwise.

COMMIT: one commit, message `J-19: TGI adapter completion`. Footer per global rule.

---

### Session J-20: SGLang adapter completion

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-14
**Files in play:** `mai/adapters/sglang/adapter.py`, `mai/adapters/sglang/client.py`, `mai/adapters/sglang/tests/test_adapter.py`, new `mai/adapters/sglang/tests/test_integration_live.py`

#### Prompt

SESSION J-20: SGLang adapter completion

CONTEXT: Bring SGLang support to completion, especially structured and constrained decoding claims.

IMPLEMENT:
1. Close every SGLang gap recorded by J-05.
2. Prove generation, streaming, structured output/constrained decoding, tool-calling if advertised, batching if advertised, health, backend unavailable, malformed response, timeout, unsupported embeddings, and shutdown cleanup.
3. Add opt-in live tests gated by `SGLANG_HOST` against a real SGLang server.
4. Update `ADAPTER-COMPLETION-MATRIX.md` SGLang row with evidence.

VERIFY:
- `pytest mai/adapters/sglang/tests -v` passes.
- `SGLANG_HOST=http://127.0.0.1:30000 pytest mai/adapters/sglang/tests/test_integration_live.py -v` passes when SGLang is running and skips cleanly otherwise.

COMMIT: one commit, message `J-20: SGLang adapter completion`. Footer per global rule.

---

### Session J-21: ExLlamaV2 adapter completion

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-14
**Files in play:** `mai/adapters/exllamav2/adapter.py`, `mai/adapters/exllamav2/client.py`, `mai/adapters/exllamav2/tests/test_adapter.py`, new `mai/adapters/exllamav2/tests/test_integration_live.py`

#### Prompt

SESSION J-21: ExLlamaV2 adapter completion

CONTEXT: Bring the quantized single-node GPU adapter to completion. This session also reinforces J-09's assertion-fill work with live coverage.

IMPLEMENT:
1. Close every ExLlamaV2 gap recorded by J-05.
2. Prove generation, streaming, hot-swap/multi-model claims if advertised, health, model missing, backend unavailable, timeout, unsupported embeddings, and shutdown cleanup.
3. Add opt-in live tests gated by `EXLLAMAV2_HOST`.
4. Update `ADAPTER-COMPLETION-MATRIX.md` ExLlamaV2 row with evidence.

VERIFY:
- `pytest mai/adapters/exllamav2/tests -v` passes.
- `EXLLAMAV2_HOST=http://127.0.0.1:5000 pytest mai/adapters/exllamav2/tests/test_integration_live.py -v` passes when backend is running and skips cleanly otherwise.

COMMIT: one commit, message `J-21: ExLlamaV2 adapter completion`. Footer per global rule.

---

### Session J-22: TensorRT-LLM/Triton adapter completion

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-14 and J-26
**Files in play:** `mai/adapters/tensorrt/adapter.py`, `mai/adapters/tensorrt/client.py`, `mai/adapters/tensorrt/tests/test_adapter.py`, new `mai/adapters/tensorrt/tests/test_integration_live.py`

#### Prompt

SESSION J-22: TensorRT-LLM/Triton adapter completion

CONTEXT: Bring the NVIDIA production adapter to completion. Hardware may be gated, but the contract cannot be vague.

IMPLEMENT:
1. Close every TensorRT-LLM/Triton gap recorded by J-05.
2. Prove generation, streaming, batching/continuous batching if advertised, model readiness, health, degraded hardware response, timeout, backend crash, malformed response, unsupported embeddings, and shutdown cleanup.
3. Add opt-in live tests gated by `TENSORRT_HOST` and `TRITON_GRPC_HOST` if needed.
4. Add deterministic local mocks for Triton responses so CI proves the adapter contract without GPU hardware.
5. Update `ADAPTER-COMPLETION-MATRIX.md` TensorRT row with evidence.

VERIFY:
- `pytest mai/adapters/tensorrt/tests -v` passes.
- Live tests pass against real Triton/TensorRT hardware when env vars are supplied and skip cleanly otherwise.

COMMIT: one commit, message `J-22: TensorRT-LLM adapter completion`. Footer per global rule.

---

### Session J-23: Generic OpenAI-compatible local adapter

**Workstream:** W3
**Depends on:** J-05
**Blocks:** J-24, J-25, J-14
**Files in play:** new `mai/adapters/openai_compat/`, tests, registry wiring

#### Prompt

SESSION J-23: Generic OpenAI-compatible local adapter

CONTEXT: Add one generic adapter for local OpenAI-compatible servers such as LM Studio, LocalAI, FastChat-style servers, or internal gateways.

IMPLEMENT:
1. Add `mai/adapters/openai_compat/` with `adapter.py`, `client.py`, `config.py`, `__init__.py`, and tests.
2. Support `/v1/models`, `/v1/completions`, `/v1/chat/completions`, `/v1/embeddings`, and streaming where the server supports SSE.
3. Config: `OPENAI_COMPAT_HOST`, optional API key, model id, timeout, max retries, and feature toggles for embeddings/streaming/tool calling.
4. Add unit tests for all method surfaces and typed errors.
5. Add opt-in live tests gated by `OPENAI_COMPAT_HOST`.
6. Update adapter registry and `ADAPTER-COMPLETION-MATRIX.md`.

VERIFY:
- `pytest mai/adapters/openai_compat/tests -v` passes.
- `OPENAI_COMPAT_HOST=http://127.0.0.1:1234 pytest mai/adapters/openai_compat/tests/test_integration_live.py -v` passes against a compatible local server and skips cleanly otherwise.

COMMIT: one commit, message `J-23: generic OpenAI-compatible adapter`. Footer per global rule.

---

### Session J-24: ONNX Runtime adapter

**Workstream:** W3
**Depends on:** J-05, J-23 fixture pattern
**Blocks:** J-14
**Files in play:** new `mai/adapters/onnxruntime/`, tests, registry wiring, optional dependency docs

#### Prompt

SESSION J-24: ONNX Runtime adapter

CONTEXT: Add CPU/DirectML/enterprise Windows fallback support through ONNX Runtime.

IMPLEMENT:
1. Add `mai/adapters/onnxruntime/` with adapter/client/config/tests.
2. Define scope honestly: local ONNX model loading, tokenizer expectations, CPU provider by default, optional DirectML/CUDA provider if installed.
3. Support generation only if the chosen ONNX model wrapper supports it; otherwise return typed unsupported errors and document model-class constraints.
4. Add unit tests with a tiny deterministic ONNX fixture or mocked session.
5. Add opt-in live tests gated by `ONNXRUNTIME_MODEL_PATH`.
6. Update adapter registry and `ADAPTER-COMPLETION-MATRIX.md`.

VERIFY:
- `pytest mai/adapters/onnxruntime/tests -v` passes.
- Live tests pass when `ONNXRUNTIME_MODEL_PATH` is supplied and skip cleanly otherwise.

COMMIT: one commit, message `J-24: ONNX Runtime adapter`. Footer per global rule.

---

### Session J-25: MLX adapter

**Workstream:** W3
**Depends on:** J-05, J-23 fixture pattern
**Blocks:** J-14
**Files in play:** new `mai/adapters/mlx/`, tests, registry wiring, optional dependency docs

#### Prompt

SESSION J-25: MLX adapter

CONTEXT: Add Apple Silicon local inference support. MLX is hardware/platform gated, so skip behavior and honest capability flags matter.

IMPLEMENT:
1. Add `mai/adapters/mlx/` with adapter/client/config/tests.
2. Config: `MLX_MODEL_PATH`, optional tokenizer path, max tokens, temperature, timeout.
3. Support generation and streaming if available through the chosen MLX interface. Embeddings/tool calling must be false unless proven.
4. Add unit tests with mocked MLX module imports so non-macOS CI remains green.
5. Add opt-in live tests gated by `MLX_MODEL_PATH` and platform checks.
6. Update adapter registry and `ADAPTER-COMPLETION-MATRIX.md`.

VERIFY:
- `pytest mai/adapters/mlx/tests -v` passes on non-Apple hardware via mocks.
- Live tests pass on Apple Silicon when `MLX_MODEL_PATH` is supplied and skip cleanly otherwise.

COMMIT: one commit, message `J-25: MLX adapter`. Footer per global rule.

---

### Session J-26: Generic Triton adapter

**Workstream:** W3
**Depends on:** J-05, J-22
**Blocks:** J-14
**Files in play:** new `mai/adapters/triton/`, tests, registry wiring

#### Prompt

SESSION J-26: Generic Triton adapter

CONTEXT: Add generic Triton inference support distinct from the TensorRT-LLM adapter. This prepares MAI for non-LLM, multimodal, embedding, classifier, and custom model workloads.

IMPLEMENT:
1. Add `mai/adapters/triton/` with adapter/client/config/tests.
2. Config: HTTP/GRPC endpoint, model name, version, input/output tensor names, timeout, readiness polling.
3. Support health/model readiness and generic inference. LLM generation surfaces must either map through configured tensor conventions or return typed unsupported errors.
4. Add local mock tests for readiness, inference, malformed tensor response, unavailable model, timeout, and shutdown cleanup.
5. Add opt-in live tests gated by `TRITON_HOST` and `TRITON_MODEL_NAME`.
6. Update adapter registry and `ADAPTER-COMPLETION-MATRIX.md`.

VERIFY:
- `pytest mai/adapters/triton/tests -v` passes.
- Live tests pass when Triton env vars are supplied and skip cleanly otherwise.

COMMIT: one commit, message `J-26: generic Triton adapter`. Footer per global rule.

---

### Session J-08: Error path audit + rate-limit + schema-validation check

**Workstream:** W4
**Depends on:** none (parallel-safe with W3 adapter sessions after J-05)
**Blocks:** —
**Files in play:** `mai/docs/ERROR-PATH-AUDIT.md` (new), plus any `mai-api/src/handlers/*.rs` files that need a fix
**Expected line delta:** doc ~150 lines; per-handler edits ±20 each (worst case 5 handlers)

#### Context Brief

John: "many critical paths lack proper error handling implementation. The error mapping is designed but not consistently applied." GitDoctor Error Handling: 60/100. Also covers SEC-011 (no rate limiting), SEC-012 (no input schema validation) audits.

#### Prompt

SESSION J-08: Error path audit + rate-limit + schema validation

CONTEXT: Trace every critical mai-api handler's error propagation; document gaps; fix the gaps in this session. Also audit rate limiting and schema validation.

IMPLEMENT:
1. mai/docs/ERROR-PATH-AUDIT.md (new, staged-write):
   - One section per mai-api handler module under mai-api/src/handlers/ (or equivalent path; discover via `git ls-files`).
   - For each handler: list the error types it can produce, whether each maps to a documented `ErrorResponse` shape, and whether the adapter-level `AdapterError` variants all have a mapping.
   - Verdict per handler: PASS / FIX-NEEDED.
2. For each FIX-NEEDED handler: apply the smallest possible Edit that brings it to PASS. Common shape: add a missing `From<AdapterError>` impl, OR change a `?` that drops context to `.map_err(|e| HandlerError::from(e).with_context(...))`.
3. Add SEC-011 audit section: list every public route, mark which have axum-level rate-limit middleware vs not. If any P0 public route is unprotected, add a rate-limit middleware in the same commit.
4. Add SEC-012 audit section: list every handler that accepts JSON, mark which use `#[serde(deny_unknown_fields)]` or equivalent schema validation. Fix the obvious gaps.

VERIFY:
- `cargo check -p mai-api` clean.
- `cargo test -p mai-api` passes (existing tests).
- The audit doc enumerates every handler in the crate (cross-check by `wc -l` of `git ls-files mai-api/src/handlers/` vs the doc's section count).
- Subagent verification required.

COMMIT: one commit per handler fixed PLUS one commit for the audit doc; OR one bundled commit `J-08: error path audit + targeted handler fixes`. Footer per global rule.

#### Acceptance Criteria

- Audit doc exists, covers every handler.
- Every FIX-NEEDED handler has a commit that brings it to PASS.
- `cargo test -p mai-api` still green.
- GitDoctor re-scan: Error Handling ≥75, SEC-011 cleared, SEC-012 cleared.

---

### Session J-09: Adapter test assertion fill (llamacpp + exllamav2)

**Workstream:** W5
**Depends on:** none
**Blocks:** J-10
**Files in play:** `mai/adapters/llamacpp/tests/test_adapter.py`, `mai/adapters/exllamav2/tests/test_adapter.py`
**Expected line delta:** llamacpp 83 → ~180; exllamav2 81 → ~180

#### Context Brief

GitDoctor `TST-004 HIGH: Test files without assertions`. Verified: llamacpp test file has 14 assertions; exllamav2 has 13; ollama is fine with 38. Grow llamacpp and exllamav2 to ≥30 each, using mocks (this session is mock-based; J-06/J-07 cover live).

#### Prompt

SESSION J-09: Fill adapter test assertions for llamacpp and exllamav2

CONTEXT: Two test files are thin. Grow each to ≥30 assertions using mocks. No live backend needed in this session — that's J-06/J-07.

IMPLEMENT:
For each of mai/adapters/llamacpp/tests/test_adapter.py and mai/adapters/exllamav2/tests/test_adapter.py:

1. Add tests covering, with ≥3 assertions each:
   - initialize: success path, malformed config raises AdapterError with code MAI-ADAPTER-INIT-..., timeout, missing model.
   - generate: happy path returns GenerationResult with completion_text/usage/finish_reason populated; non-existent model raises ModelNotFoundError; empty prompt raises clean ValidationError; cancelled mid-generation handled.
   - stream: token events arrive, FinishReason set on done event, usage accounting correct.
   - health: ok status, degraded status, unreachable status.
   - capabilities: returns AdapterCapabilities with all booleans set deterministically per backend.
   - shutdown: idempotent (second call no-op), pending requests cancelled, resources released.

2. Use the existing client mock pattern (look at adapters/ollama/tests/test_adapter.py for the pattern).

3. Total assertion count per file ≥30 (verify with `grep -c '^\s*assert\|pytest.raises' <file>`).

VERIFY:
- `pytest mai/adapters/llamacpp/tests/test_adapter.py -v` passes.
- `pytest mai/adapters/exllamav2/tests/test_adapter.py -v` passes.
- Assertion count check ≥30 each.
- Edit tool only (atomic patches), no Write since both files exist.

COMMIT: one commit per file or one bundled `J-09: fill adapter test assertions for llamacpp and exllamav2`. Footer per global rule.

#### Acceptance Criteria

- Both files ≥30 assertions.
- Both test suites pass.
- GitDoctor re-scan: TST-004 cleared.

---

### Session J-10: Pytest assertion-count gate + e2e compliance smoke

**Workstream:** W5
**Depends on:** J-09
**Blocks:** —
**Files in play:** `mai/tests/integrity/test_assertion_gate.py` (new), `mai/tests/e2e/test_compliance_smoke.py` (new), `mai/pyproject.toml` (edit if new test path needs registering)
**Expected line delta:** gate ~60 lines, smoke ~80 lines

#### Context Brief

Codify the "≥3 assertions per test file" rule as a CI gate, and add a single Python end-to-end smoke that exercises the compliance demo path (audit log + report gen + router) from outside the Rust workspace.

#### Prompt

SESSION J-10: Pytest assertion gate + e2e compliance smoke

CONTEXT: Codify the assertion-floor rule; add Python e2e covering compliance demos.

IMPLEMENT:
1. mai/tests/integrity/test_assertion_gate.py (new, staged-write):
   - Walk every `test_*.py` under mai/adapters/ and mai/tests/.
   - For each, count lines starting with `assert` or `pytest.raises`.
   - Fail if any test_*.py has < 3 assertions (ignore __init__.py and conftest.py).
   - Excludes the gate file itself.

2. mai/tests/e2e/test_compliance_smoke.py (new, staged-write):
   - Spawn the mai-api binary (cargo run --release -p mai-api in subprocess, or use a built binary if present).
   - Wait for /health = 200.
   - Hit /v1/route with a healthcare-flavoured query, assert routing decision == local AND policy_decision.layer == "HIPAA".
   - Hit /v1/audit/recent, assert at least one event recorded for the previous routing call AND that event has a hash-chain prev_hash matching the previous head.
   - Hit /v1/audit/report?range=last-hour, assert it returns within 100ms AND contains a non-zero event count.
   - Tear down the spawned process cleanly.

3. mai/pyproject.toml: ensure the new test paths are discoverable (add to `[tool.pytest.ini_options].testpaths` if needed).

VERIFY:
- `pytest mai/tests/integrity/test_assertion_gate.py` passes (i.e., no test file violates the floor after J-09).
- `pytest mai/tests/e2e/test_compliance_smoke.py` passes against a running mai-api.
- Subagent verification required.

COMMIT: one commit, message `J-10: pytest assertion gate + e2e compliance smoke`. Footer per global rule.

#### Acceptance Criteria

- Assertion gate test passes.
- E2E smoke passes against the locally built mai-api.
- GitDoctor re-scan: TST-005 cleared (now has e2e), TST-001 cleared.

---

### Session J-11: Refactor MCP server.js into focused modules

**Workstream:** W6 (Code Hygiene)
**Depends on:** J-01 (the security patch lands first so this refactor preserves it)
**Blocks:** —
**Files in play:** `mai/.integrity/mcp-server/server.js` (shrink to ~150 LOC), `mai/.integrity/mcp-server/handlers.js` (new), `mai/.integrity/mcp-server/validators.js` (new), `mai/.integrity/mcp-server/logger.js` (new), `mai/.integrity/mcp-server/package.json` (edit to add logger dep)
**Expected line delta:** server.js -220, +three new files (~80 each)

#### Context Brief

GitDoctor: QUA-001 (371-line god file), QUA-009 (4-level nesting), PERF-004 (JSON.stringify in loop), QUA-005 (excessive console.log), CFG-006 (no strict mode). One refactor session addresses all five.

#### Prompt

SESSION J-11: Refactor MCP server.js into focused modules

CONTEXT: Address QUA-001 + QUA-009 + PERF-004 + QUA-005 + CFG-006 in one refactor. Preserve all functional behaviour (this is the file the workspace integrity tooling depends on — DO NOT break it).

IMPLEMENT (each new file via staged-write):
1. mai/.integrity/mcp-server/logger.js (new, ~30 lines): thin wrapper around `pino` (or `bunyan` if pino is unwelcome). Exposes `log.info`, `log.warn`, `log.error`. Pino is the recommended choice — fast, low-dep, JSON output suitable for the integrity tooling's audit needs.
2. mai/.integrity/mcp-server/validators.js (new, ~100 lines): pull out the per-check validation functions currently inlined in server.js (line-count check, tail check, null-byte check, bracket balance).
3. mai/.integrity/mcp-server/handlers.js (new, ~120 lines): pull out the per-tool handler bodies (validate_file, safe_write, post_write_verify, verify_tree).
4. server.js (shrink to ~150 lines): keep only bootstrap, setRequestHandler wiring, error envelope, shutdown. All console.log calls replaced with `log.info/warn/error`. The JSON.stringify-in-loop at line 317 lifted out: build the response object inside the loop, stringify ONCE after the loop completes.
5. Reduce nesting at line 69 (currently 4 levels) by extracting early-return helpers.
6. mai/.integrity/mcp-server/package.json (edit): add `"pino": "^9.0.0"` to dependencies. Re-run `npm install --package-lock-only` to refresh package-lock.json.
7. Optional: add `// @ts-check` to each .js file and JSDoc type annotations to the exported functions in handlers.js / validators.js. This addresses CFG-006 without a full TS migration.

VERIFY (CRITICAL — this is the integrity tooling itself):
- `node --check` on every modified/new .js file.
- Smoke: launch the server, call `validate_file` against a known-good file (e.g. mai/adapters/ollama/adapter.py with expected_lines=316), assert the response matches what the pre-refactor server returned (capture pre-refactor output FIRST).
- `wc -l server.js` ≤200 (target ~150).
- Subagent verification REQUIRED (4-5 files).

COMMIT: one commit, message `J-11: split MCP server into handlers/validators/logger, lift JSON stringify out of loop, structured logging`. Footer per global rule.

#### Acceptance Criteria

- server.js ≤200 lines.
- Three new modules each ≤150 lines.
- All console.log replaced.
- JSON.stringify lifted out of loop.
- Smoke against the integrity tooling matches pre-refactor output byte-for-byte.
- GitDoctor re-scan: QUA-001, QUA-009, PERF-004, QUA-005, CFG-006 all cleared.

---

### Session J-12: Async context managers on AdapterBase and concrete adapters

**Workstream:** W7
**Depends on:** none (parallel-safe)
**Blocks:** J-13
**Files in play:** `mai/adapters/base.py` (edit), `mai/adapters/{ollama,llamacpp,exllamav2,vllm,tgi,sglang,tensorrt}/adapter.py` (small edits, ~5 lines each)
**Expected line delta:** base.py +15, each adapter +0 (inheritance) — but a smoke test per adapter

#### Context Brief

John's email: "Add proper async context managers for adapter lifecycle to ensure clean resource management and connection cleanup." Today: `initialize` and `shutdown` are manual. Add `__aenter__` / `__aexit__` on `AdapterBase` that delegate. Subclasses inherit; no per-adapter changes needed unless one needs a custom enter/exit (none currently do).

#### Prompt

SESSION J-12: Async context managers on AdapterBase

CONTEXT: Today adapters use manual initialize()/shutdown(). Add __aenter__/__aexit__ so `async with OllamaAdapter() as a: ...` works.

IMPLEMENT:
1. mai/adapters/base.py — Edit only (file exists):
   - Add two methods to AdapterBase:
     ```python
     async def __aenter__(self):
         # Caller must have set config via a prior mechanism; if none, raise AdapterError.
         if self._config is None:
             raise AdapterError("config not set; pass config via constructor or set_config() before async with")
         await self.initialize(self._config, hil_handle=self._hil_handle)
         return self

     async def __aexit__(self, exc_type, exc_val, exc_tb):
         try:
             await self.shutdown()
         finally:
             return False  # do not suppress exceptions
     ```
   - If AdapterBase does not currently store _config / _hil_handle, add a `set_config(config, hil_handle)` method to set them, called before `async with`. This keeps the public API simple.

2. Per-adapter smoke: add one test per adapter in its `tests/test_adapter.py` verifying that `async with Adapter() as a:` works and that shutdown is called. Use existing client mocks (no live backend).

VERIFY:
- `pytest mai/adapters/` passes.
- Subagent verification required (8 files touched).

COMMIT: one commit, message `J-12: async context manager support on AdapterBase`. Footer per global rule.

#### Acceptance Criteria

- `async with Adapter() as a:` works on every backend.
- Mock-based smoke test per backend.
- All existing adapter tests still pass.

---

### Session J-13: /health/system aggregator endpoint

**Workstream:** W7
**Depends on:** J-12 (no real dep, but co-locates lifecycle work)
**Blocks:** J-14
**Files in play:** `mai/mai-api/src/handlers/health.rs` (likely new submodule or section), one integration test
**Expected line delta:** handler ~80, test ~60

#### Context Brief

John: "Create a health check aggregator that monitors all active adapters and provides system-wide health status for production monitoring." Today: per-adapter `health()`. Add `/health/system` that fans out and returns a JSON rollup.

#### Prompt

SESSION J-13: /health/system aggregator endpoint

CONTEXT: Need a system-wide health rollup endpoint for production monitoring.

IMPLEMENT:
1. mai/mai-api/src/handlers/health.rs (edit, or new file if no existing health module — discover via `git ls-files mai-api/src/handlers/`):
   - Add an axum route handler GET /health/system.
   - The handler reads the registered adapter set from the AdapterManager.
   - Fan out: call `health()` on each adapter concurrently (futures::join_all or tokio::join).
   - Build a JSON response:
     ```json
     {
       "overall": "ok" | "degraded" | "down",
       "adapters": {
         "ollama": { "status": "ok", "latency_ms": 12, "model_loaded": true, ... },
         "llamacpp": { ... }
       },
       "ts": "<RFC3339>"
     }
     ```
   - Overall = "ok" if all ok; "degraded" if any degraded; "down" if any down or unreachable.

2. Wire the route in the axum Router (next to the existing /health route).

3. Integration test in mai/mai-api/tests/ (new): spin up the test server with mock adapters, call /health/system, assert the structure and that overall correctly reflects per-adapter status.

VERIFY:
- `cargo test -p mai-api` passes.
- `cargo clippy -p mai-api` clean.
- Hit /health/system manually with `curl` against a local mai-api, get the expected JSON.

COMMIT: one commit, message `J-13: /health/system aggregator endpoint`. Footer per global rule.

#### Acceptance Criteria

- Endpoint exists, returns the documented JSON.
- Integration test passes.
- GitDoctor re-scan: CFG-003 cleared.

---

### Session J-16: mai-sdk-rs HTTP client implementation

**Workstream:** W10 (SDK)
**Depends on:** none (parallel-safe with all W3/W5/W6/W7 work)
**Blocks:** J-17, J-14
**Files in play:** `mai/mai-sdk-rs/src/lib.rs` (large Edit — 14 method bodies), `mai/mai-sdk-rs/Cargo.toml` (add `reqwest` + dev-deps), `mai/mai-sdk-rs/tests/http_client.rs` (new, staged-write), optionally split `lib.rs` into `client.rs` + `lib.rs` if size warrants
**Expected line delta:** lib.rs ~+250 (14 bodies × ~15 LOC + helpers), tests ~+200, Cargo.toml +4

#### Context Brief

`mai/mai-sdk-rs/src/lib.rs` is 999 lines and contains 17 `todo!()` sites between lines 768 and 887, all labeled `"Session 11: HTTP client"` / `"SSE streaming"` / `"resume protocol"`. The crate ships in the RC1 bundle. This session implements the 14 plain-HTTP method bodies (everything except SSE and resume — those are J-17). Per workspace `CLAUDE.md` §"Sandbox Disk Space Rules", run `df -h /` before adding dependencies and `cargo clean` after the build if `target/` grows past 1.5 GB.

The architectural carve-out: `mai-sdk-rs` is consumed OUTSIDE the air-gap by L4-L5 application scaffolds. Pulling `reqwest` is consistent with the threat model — the air-gap "stdlib only" rule applies to the inference + compliance core, not the SDK that talks TO it.

#### Prompt

SESSION J-16: mai-sdk-rs HTTP client implementation

CONTEXT: 17 todo!() sites in mai/mai-sdk-rs/src/lib.rs (lines 768-887). This session
implements the 14 plain-HTTP methods. SSE streaming (3 sites: stream/stream_resume/
resume at lines 882/887/etc) is J-17 — leave those todo!()s in place.

PRE-FLIGHT (mandatory):
- `df -h /` — verify ≥2 GB free before adding reqwest.
- `wc -l mai/mai-sdk-rs/src/lib.rs` — record baseline (expect 999).
- `grep -c 'todo!' mai/mai-sdk-rs/src/lib.rs` — record baseline (expect 17).
- `git show HEAD:mai/mai-sdk-rs/src/lib.rs | head -5` — confirm canonical pre-edit state.

IMPLEMENT:

1. mai/mai-sdk-rs/Cargo.toml — Edit only:
   Add under [dependencies] (pin to workspace versions if reqwest is already a workspace dep; otherwise pin exact version):
     reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
     url = "2.5"
     anyhow = "1"  # or stay with thiserror::Error if a custom error enum is preferred
   Add under [dev-dependencies]:
     wiremock = "0.6"
     tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
   Rationale comment in the file header: "rustls-tls keeps OpenSSL out of the dep tree;
   reqwest is consumed outside the air-gap so this is consistent with the threat model."

2. mai/mai-sdk-rs/src/lib.rs — Edit (atomic patches, one method at a time):
   For each todo!("Session 11: HTTP client") site at lines 768/782/787/792/800/807/812/819/824/829/836/844/851/862, replace the body with a real reqwest call that:
   - Constructs the URL from self.base_url + the route.
   - Sets Authorization header from self.auth_token if present.
   - Serializes the request body via serde_json::to_vec.
   - Sends via self.http (a reqwest::Client stored on the struct, hoisted from a new() helper — add one if not present).
   - Reads the response status, maps non-2xx to SdkError variants matching the existing taxonomy.
   - Deserializes the response body via serde_json::from_slice.
   - Returns Result<ResponseType, SdkError>.
   Each method body is small (~10-15 lines). Do NOT collapse multiple methods into a generic helper unless ≥4 of them share an identical shape — premature abstraction is worse than three similar bodies.

3. If a `Client::new(base_url: impl Into<String>, auth_token: Option<String>) -> Self` constructor does not exist, add one. It must create the reqwest::Client with a sane default timeout (30s) and a connection pool (reqwest default is fine).

4. mai/mai-sdk-rs/tests/http_client.rs — NEW (staged-write protocol mandatory, >40 lines):
   - Per-method test using wiremock.
   - Test shape: spin up wiremock::MockServer, register an expected request+response, instantiate Client pointing at the mock URL, call the method, assert response matches.
   - Cover: happy path per method (14 tests), one auth-required-but-missing test, one 5xx-error-mapping test, one network-timeout test, one malformed-JSON-response test. Each test must have ≥3 assertions.

5. SHIP KNOWN-ISSUES.md — Edit:
   - Update Issue 15 entry: change status to "closing in J-16/J-17, awaiting J-17 commit hash".

VERIFY (mandatory before commit):
- `cargo check -p mai-sdk-rs` clean.
- `cargo clippy -p mai-sdk-rs -- -D warnings` clean.
- `cargo test -p mai-sdk-rs` passes (new tests + any pre-existing).
- `grep -c 'todo!' mai/mai-sdk-rs/src/lib.rs` == 3 (only the SSE sites remain for J-17).
- `wc -l mai/mai-sdk-rs/src/lib.rs` within expected delta (baseline + ~250).
- Subagent verification REQUIRED (3+ files modified/created).
- Disk: `du -sh mai/target/` — if >1.5 GB after build, `cargo clean -p mai-sdk-rs` before commit.

COMMIT: one commit, message `J-16: mai-sdk-rs HTTP client implementation (14 of 17 todo! sites closed)`. Footer per global rule.

#### Acceptance Criteria

- 14 of 17 `todo!()` sites in lib.rs replaced with real reqwest bodies.
- 3 SSE-related `todo!()` sites remain intentionally (J-17 fixture).
- `mai/mai-sdk-rs/tests/http_client.rs` exists with ≥14 happy-path tests + ≥4 edge-case tests.
- Cargo.toml carries pinned reqwest + wiremock.
- `cargo test -p mai-sdk-rs` passes.
- `cargo clippy -p mai-sdk-rs -- -D warnings` clean.
- KNOWN-ISSUES.md Issue 15 updated with J-16 commit hash.

---

### Session J-17: mai-sdk-rs SSE streaming + resume protocol

**Workstream:** W10 (SDK)
**Depends on:** J-16 (reqwest dep + Client struct + auth header pattern)
**Blocks:** J-14
**Files in play:** `mai/mai-sdk-rs/src/lib.rs` (3 method bodies — `stream`, `stream_resume`, `resume`), `mai/mai-sdk-rs/Cargo.toml` (add SSE dep), `mai/mai-sdk-rs/tests/streaming.rs` (new, staged-write)
**Expected line delta:** lib.rs ~+100, tests ~+150, Cargo.toml +1-3

#### Context Brief

After J-16 closes the 14 plain-HTTP `todo!()`s, three streaming-flavored sites remain at lines ~882 (`todo!("Session 11: SSE stream")`) and ~887 (`todo!("Session 11: resume protocol")`) plus one more at line ~768-range for `stream`. This session adds SSE event parsing, a stream() method that yields token events, and a resume() method that takes a `last_event_id` and re-subscribes from that point.

The reference protocol is the one the mai-api server emits: NDJSON token events with `id`, `event`, `data` fields per the SSE spec (see `mai/docs/IPC-PROTOCOL.md` and the api crate's streaming handler for the canonical shape).

#### Prompt

SESSION J-17: mai-sdk-rs SSE streaming + resume protocol

CONTEXT: J-16 left 3 SSE-related todo!() sites in mai/mai-sdk-rs/src/lib.rs. Close them.

PRE-FLIGHT:
- `df -h /` — verify ≥1.5 GB free.
- `grep -n 'todo!' mai/mai-sdk-rs/src/lib.rs` — confirm exactly 3 sites remain.
- Read mai/docs/IPC-PROTOCOL.md to confirm the canonical SSE event shape.
- Skim mai-api/src/handlers/ for the streaming response producer to align field names.

DECISION POINT — pick ONE:
A) eventsource-client crate (~10 transitive deps, well-maintained, opinionated)
B) Hand-rolled SSE parser over reqwest::Response::bytes_stream (zero new deps beyond what J-16 added, ~80 LOC parser)
Default: (B) hand-rolled. Justification: keeps the dep tree small AND matches the workspace's stdlib-leaning style. Choose (A) only if Basho explicitly requests it.

IMPLEMENT:

1. mai/mai-sdk-rs/Cargo.toml — Edit only:
   If decision = A: add `eventsource-client = "0.13"` to [dependencies].
   If decision = B: add `futures-util = "0.3"` and `tokio-stream = "0.1"` to [dependencies] (used for stream combinators).

2. mai/mai-sdk-rs/src/lib.rs — Edit only:
   Replace the 3 todo!() sites with:
   - `stream(&self, req: GenerateRequest) -> impl Stream<Item = Result<TokenEvent, SdkError>>`:
     POST the request with Accept: text/event-stream, parse the SSE response, yield TokenEvent per event. Track the last event id internally so resume() works.
   - `resume(&self, stream_id: StreamId, last_event_id: EventId) -> impl Stream<...>`:
     GET /v1/stream/{stream_id}?last_event_id={...}, parse SSE, yield from there.
   - `stream_resume(&self, req: GenerateRequest) -> impl Stream<...>` (convenience wrapper that internally retries on disconnect and uses resume()).

3. mai/mai-sdk-rs/tests/streaming.rs — NEW (staged-write protocol):
   - Test against wiremock with a streaming response (wiremock supports body_stream).
   - Tests:
     - `stream_yields_events_in_order`: 5 events sent, 5 events received in order. ≥3 assertions.
     - `stream_handles_disconnect_mid_stream`: connection killed after event 3, expect a stream-level error event. ≥3 assertions.
     - `resume_picks_up_after_last_event_id`: simulate first stream getting events 1-3, then resume from event-id=3 and assert events 4-5 arrive. ≥3 assertions.
     - `stream_resume_auto_retries`: first connection drops after event 3, stream_resume helper should auto-reconnect and yield 4-5 transparently. ≥3 assertions.
     - `malformed_sse_event_returns_error`: send a non-SSE response body, expect SdkError::Protocol with a descriptive message. ≥3 assertions.

4. SHIP KNOWN-ISSUES.md — Edit:
   - Mark Issue 15 CLOSED, with both J-16 and J-17 commit hashes.

VERIFY:
- `cargo check -p mai-sdk-rs` clean.
- `cargo clippy -p mai-sdk-rs -- -D warnings` clean.
- `cargo test -p mai-sdk-rs` passes — all tests (J-16 + J-17 streaming).
- `grep -c 'todo!' mai/mai-sdk-rs/src/lib.rs` == 0.
- Subagent verification REQUIRED.
- Disk: `du -sh mai/target/`; cargo clean if >1.5 GB.

COMMIT: one commit, message `J-17: mai-sdk-rs SSE streaming + resume protocol (Issue 15 CLOSED)`. Footer per global rule.

#### Acceptance Criteria

- Zero `todo!()` in `mai/mai-sdk-rs/src/lib.rs`.
- `mai/mai-sdk-rs/tests/streaming.rs` exists with ≥5 tests, ≥3 assertions each.
- `cargo test -p mai-sdk-rs` passes including streaming tests.
- KNOWN-ISSUES.md Issue 15 marked CLOSED with both commit hashes.
- `cargo doc -p mai-sdk-rs --no-deps` renders documentation for stream/resume methods with no remaining `todo!()` warnings.

---

### Session J-14: Re-run GitDoctor

**Workstream:** W9
**Depends on:** J-01..J-13, J-16..J-17, and J-18..J-26 all committed and pushed to origin/main
**Blocks:** J-15
**Files in play:** evidence only — `mai/test-evidence/dougherty-rescan/*.png` (screenshots)
**Expected line delta:** evidence directory ~15 files

#### Context Brief

After all fix sessions land, re-run the GitDoctor scan against the post-DOUGHERTY HEAD. Capture all result screens. Compare against the original screenshots in `.tester-feedback-2026-05-24/`.

#### Prompt

SESSION J-14: Re-run GitDoctor and capture evidence

CONTEXT: J-01..J-13, J-16..J-17, and the full W3 adapter matrix J-18..J-26 are committed. Re-run the scan and capture results.

IMPLEMENT:
1. Ensure all DOUGHERTY commits are pushed to origin/main (this is what GitDoctor scans).
2. Navigate to https://gitdoctor.io and scan `USS-Parks/im-mighty-eel-mai`. Wait for completion.
3. Screenshot every tab/panel that the 2026-05-24 scan captured (15 screenshots). Save to mai/test-evidence/dougherty-rescan/.
4. Create mai/test-evidence/dougherty-rescan/SUMMARY.md (~40 lines, staged-write): per-category score before/after table, list of resolved findings, list of remaining findings (and which lane addresses them — likely "out of scope, see DOUGHERTY plan §8").

VERIFY:
- 15 PNGs present in the directory.
- SUMMARY.md tail matches intent.

COMMIT: one commit, message `J-14: GitDoctor re-scan evidence after DOUGHERTY lane`. Footer per global rule.

#### Acceptance Criteria

- Evidence directory has all screenshots.
- SUMMARY.md compares scores.
- Overall ≥75. Zero HIGH security findings. Zero HIGH project findings.
- If overall < 75: STOP. Open a J-14b session to identify which gate fix slipped and re-run.

---

### Session J-15: Tester response doc + RC-10 prep

**Workstream:** W8 (Evidence Pack) + W9 (Close)
**Depends on:** J-14
**Blocks:** RC-10
**Files in play:** `mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md` (new, comprehensive), `mai/docs/RC1-TESTER-FEEDBACK.md` (edit § for John), update to `project_rc_release_lane` memory
**Expected line delta:** response doc ~250 lines

#### Context Brief

The lane closes here. Single document, sent to John (drafted in workspace for Basho to send by email/portal). Every John item answered with verdict + evidence + (if applicable) commit hash.

#### Prompt

SESSION J-15: Tester response doc + RC-10 prep

CONTEXT: Final session of the DOUGHERTY lane. Draft the response John receives. Update lane memory.

IMPLEMENT:
1. mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md (new, staged-write protocol, ~250 lines):
   - §1 What we fixed (table: John item → J-session → commit hash → before/after).
   - §2 What we measured (GitDoctor rescan score deltas; link to test-evidence/dougherty-rescan/SUMMARY.md).
   - §3 What we deferred and why (the dashboard item; the stdlib-not-third-party decision).
   - §4 What we believe the scan (and the email paraphrase) got wrong (with evidence pastes):
     - Adapter stubs: `wc -l` of adapters/*/adapter.py + `grep -c NotImplementedError` = 0 for each. **Important nuance:** John's "extensive TODO placeholders" claim was correct WHEN APPLIED TO `mai-sdk-rs/src/lib.rs` (17 `todo!()` sites, closed in J-16/J-17) but wrong when applied to the Python adapter layer. The response doc must make this distinction explicitly — do NOT broad-brush refute "stubs exist" because the SDK ones were real. Cite the J-16 and J-17 commits as the SDK fix.
     - Flat structure: `tree -L 2 mai/` output showing crates/, adapters/, docs/, tests/, .integrity/.
     - Stdlib-only: ARCHITECTURE.md citation + threat-model paragraph quoted verbatim. Add a line noting that the SDK (mai-sdk-rs) lives OUTSIDE the air-gap boundary and intentionally pulls reqwest as of J-16 — the stdlib-only rule applies to the inference + compliance core, not the SDK that talks to it.
   - §5 What's next (RC-10: re-bundle and re-ship; we invite a re-test).
   - §6 Thanks line — John caught real things (the HIGH Math.random, the missing Dockerfile, the lock files, the thin tests). Acknowledge.

2. mai/docs/RC1-TESTER-FEEDBACK.md (edit) — update §9 from "open" to "responded — awaiting tester re-test", reference the response doc and the J-session commits.

3. Update memory file project_rc_release_lane.md (workspace memory) — append a bullet: "DOUGHERTY lane CLOSED <date>; J-01..J-26 all committed; GitDoctor overall <new-score>/100; response doc at mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md; next: RC-10 re-bundle."

VERIFY:
- Response doc tail matches intent.
- Every J-session referenced by a real commit hash from `git log`.
- Subagent verification required.

COMMIT: one commit, message `J-15: DOUGHERTY lane response doc + lane closure`. Footer per global rule.

#### Acceptance Criteria

- Response doc exists and addresses every John item.
- RC1-TESTER-FEEDBACK.md updated.
- Memory updated.
- The doc is ready to send (Basho sends, not Claude).
- RC-10 prerequisites declared met in the commit message.

---

## Session-by-session verification matrix (summary)

| Session | Subagent verify required? | New files >40 LOC? | Lock-files / staged-write | Edit-only safe? |
|:--------|:--:|:--:|:--:|:--:|
| J-01    | no  | no  | n/a | YES (Edit only) |
| J-02    | no  | no  | n/a | YES |
| J-03    | YES | YES (LOCK-FILE-POLICY.md) | staged | mixed |
| J-04    | YES | YES (Dockerfile + .dockerignore + .env.example) | staged | new |
| J-05    | no  | YES (ADAPTER-COMPLETION-MATRIX.md) | staged | new |
| J-06    | YES | YES (test_integration_live.py) | staged | mixed |
| J-07    | YES | YES (test_integration_live.py) | staged | mixed |
| J-18    | YES | YES (test_integration_live.py) | staged if new | mixed |
| J-19    | YES | YES (test_integration_live.py) | staged if new | mixed |
| J-20    | YES | YES (test_integration_live.py) | staged if new | mixed |
| J-21    | YES | YES (test_integration_live.py) | staged if new | mixed |
| J-22    | YES | YES (test_integration_live.py) | staged if new | mixed |
| J-23    | YES | YES (new adapter + tests) | staged | new |
| J-24    | YES | YES (new adapter + tests) | staged | new |
| J-25    | YES | YES (new adapter + tests) | staged | new |
| J-26    | YES | YES (new adapter + tests) | staged | new |
| J-08    | YES | YES (ERROR-PATH-AUDIT.md) | staged | mixed |
| J-09    | no  | no | n/a | YES (Edit only — files exist) |
| J-10    | YES | YES (gate + smoke) | staged | new |
| J-11    | YES | YES (handlers.js + validators.js + logger.js) | staged | mixed |
| J-12    | YES | no | n/a | YES |
| J-13    | YES | maybe (health.rs may be new) | staged if new | mixed |
| J-16    | YES | YES (tests/http_client.rs) | staged | mixed (Edit lib.rs/Cargo.toml; new tests file) |
| J-17    | YES | YES (tests/streaming.rs) | staged | mixed |
| J-14    | no  | YES (SUMMARY.md) | staged | new |
| J-15    | YES | YES (response doc) | staged | new |

---

## Done-criteria for the entire lane

The DOUGHERTY lane is CLOSED when every box ticks:

- [x] J-01 committed (Math.random replaced; `6621c02`).
- [x] J-02 committed (gitignore patched; `be7a347`).
- [x] J-03 committed (lock files + policy doc; `468e0e8`).
- [x] J-04 committed (Dockerfile + dockerignore + env example; `2cdc23a`, fix-up `e32d8fe`).
- [x] J-05 committed (adapter completion matrix + pooling audit doc; `63a0327`).
- [x] J-06 committed (Ollama live tests passing locally; `c92918c`).
- [x] J-07 committed (llama.cpp live tests passing locally; `3fa93ce`).
- [x] J-18 committed (vLLM adapter completion; `66eaacd`).
- [x] J-19 committed (TGI adapter completion; `8f1ac4d`).
- [x] J-20 committed (SGLang adapter completion; `339d798`).
- [x] J-21 committed (ExLlamaV2 adapter completion; `ce7ea52`).
- [x] J-22 committed (TensorRT-LLM/Triton adapter completion; `58e7394`).
- [ ] J-23 committed (generic OpenAI-compatible local adapter).
- [x] J-24 committed (ONNX Runtime adapter; `74be424`).
- [x] J-25 committed (MLX adapter; `84cfaf6`, cleanup `4c45754`).
- [ ] J-26 committed (generic Triton adapter).
- [x] J-08 committed (error path audit + targeted fixes; `606e821`).
- [x] J-09 committed (llamacpp + exllamav2 tests >=30 assertions; `d18da96`, addendum `182e075`).
- [x] J-10 committed (assertion gate + e2e smoke; `2a7bced`, evidence fix `6bb6dbc`).
- [x] J-11 committed (MCP server refactor; `5f14f6a`).
- [x] J-12 committed (async context managers; `72533ea`, cleanup `233d85c`).
- [x] J-13 committed (/health/system; `99bfd5a`).
- [x] J-16 committed (mai-sdk-rs HTTP client; zero `todo!()` in `mai-sdk-rs/src/lib.rs`; tests/http_client.rs added; `88fa06e` captures the wiremock suite).
- [x] J-17 functional acceptance closed in the current Rust SDK source: streaming/resume bodies are implemented and `rg -n "todo!" mai-sdk-rs/src/lib.rs` returns no matches. No standalone `tests/streaming.rs` commit is visible in the current history; keep this nuance in the J-14/J-15 response evidence.
- [ ] J-14 committed (re-scan evidence — runs AFTER J-13, J-17, and J-26).
- [ ] J-15 committed (response doc + memory update).
- [ ] Every commit carries the canonical co-author footer.
- [ ] GitDoctor overall ≥75, zero HIGH security findings, zero HIGH project findings.
- [ ] `mai/docs/RC1-TESTER-RESPONSE-DOUGHERTY.md` ready to send.
- [ ] RC-10 declared unblocked.
