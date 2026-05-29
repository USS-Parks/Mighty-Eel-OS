# Local GitDoctor-Style Audit

Root: `C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai`
Overall score: **59/100**
Checks: 58 total, 34 passed, 24 failed

## Category Scores

| Category | Score | Passed | Failed |
|---|---:|---:|---:|
| Code Quality | 30/100 | 3 | 7 |
| Configuration | 71/100 | 5 | 2 |
| Performance | 50/100 | 3 | 3 |
| Project Hygiene | 80/100 | 4 | 1 |
| Review Integrity | 25/100 | 2 | 6 |
| Security | 81/100 | 13 | 3 |
| Testing | 67/100 | 4 | 2 |

## Findings

### SEC-003 SQL injection via string interpolation (HIGH)

Layer: `mapped-check`  
Origin: `john-finding`

Interpolated SQL can create injection vulnerabilities.

- `compliance-dashboard/reports.py:5 ``list_reports`` / ``get_report`` / ``delete_report`` /`
- `mai-api/src/grpc/server.rs:68 /// the REST server in a `tokio::select!` or `tokio::join!`.`
- `mai-api/src/handlers/compliance.rs:26 //! - `DELETE /v1/compliance/reports/{id}`       — delete (refuses protected)`
- `mai-api/src/handlers/compliance.rs:647 /// `DELETE /v1/compliance/reports/{id}``
- `mai-api/src/sealer_builder.rs:3 //! Select the [`StoreSealer`] implementation for the compliance audit`
- `mai-api/src/state.rs:78 /// SHIP-07 Slice B: selected `POST /v1/auth/exchange_token` mode.`
- `mai-api/src/trust_builder.rs:10 //!   Disabled) — selects how `POST /v1/auth/exchange_token` mints`
- `mai-api/src/trust_builder.rs:78 /// Selected behavior for `POST /v1/auth/exchange_token`.`

### SEC-004 Hardcoded API keys and secrets (HIGH)

Layer: `mapped-check`  
Origin: `john-finding`

Secrets should not be committed to source.

- `compliance-dashboard/util.py:21 ENV_API_TOKEN = "MAI_DASHBOARD_API_TOKEN"`
- `compliance-dashboard/util.py:22 ENV_ADMIN_TOKEN = "MAI_DASHBOARD_ADMIN_TOKEN"`
- `tests/sdk_integration.py:71 api_key="im-this-key-does-not-exist-at-all",`

### SEC-016 State-changing GET routes (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

GET endpoints should not mutate server state.

- `compliance-dashboard/reports.py:5 ``list_reports`` / ``get_report`` / ``delete_report`` /`
- `mai-api/src/audit.rs:440 /// Get the writer reference for direct queries.`
- `mai-api/src/config.rs:401 /// Get a receiver that yields updated configs when the file changes.`
- `mai-api/src/handlers/updates.rs:28 /// GET /v1/updates/check`
- `mai-api/src/handlers/updates.rs:97 /// GET /v1/updates/status`
- `mai-api/src/routes.rs:70 get(handlers::models::get_model).delete(handlers::models::remove_model_handler),`
- `mai-api/src/routes.rs:104 .route("/v1/updates/check", get(handlers::updates::check_updates))`
- `mai-api/src/routes.rs:109 .route("/v1/updates/status", get(handlers::updates::update_status));`

### PERF-002 Synchronous file I/O blocking event loop (LOW)

Layer: `mapped-check`  
Origin: `john-finding`

Sync Node file I/O blocks the event loop.

- `.integrity/mcp-server/server.js:20 import { readFileSync, writeFileSync, mkdirSync, copyFileSync, existsSync, statSync } from "fs";`
- `.integrity/mcp-server/server.js:111 const content = readFileSync(filePath);`
- `.integrity/mcp-server/server.js:120 const content = readFileSync(filePath, "utf-8");`
- `.integrity/mcp-server/server.js:138 const content = readFileSync(filePath, "utf-8");`
- `.integrity/mcp-server/server.js:154 const currentContent = readFileSync(filePath, "utf-8");`
- `.integrity/mcp-server/server.js:177 if (!existsSync(filePath)) {`
- `.integrity/mcp-server/server.js:202 const content = readFileSync(filePath, "utf-8");`
- `.integrity/mcp-server/server.js:217 const stat = statSync(filePath);`

### PERF-003 N+1 database query pattern (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

Database calls inside loops can create N+1 behavior.

- `compliance-dashboard/tests/test_dashboard.py:22 sys.path.insert(0, str(path))`
- `mai-adapters/src/manager.rs:83 processes.insert(adapter.name.clone(), Mutex::new(process));`
- `mai-adapters/src/process.rs:446 full_obj.insert(k.clone(), v.clone());`
- `mai-agent/src/context.rs:261 session.segments.insert(insert_pos, segment);`
- `mai-agent/src/rag.rs:299 payload.insert(`
- `mai-agent/src/rag.rs:303 payload.insert(`
- `mai-api/src/auth.rs:686 assert!(keys.insert(generate_api_key()));`
- `mai-api/src/production_guard.rs:245 seen.insert(c.id.as_str()),`

### PERF-004 JSON.parse/stringify inside loops (LOW)

Layer: `mapped-check`  
Origin: `john-finding`

Repeated JSON serialization in loops adds CPU overhead.

- `adapters/exllamav2/client.py:116 chunk_data = json.loads(payload)`
- `adapters/llamacpp/client.py:121 chunk_data = json.loads(payload)`
- `adapters/ollama/client.py:147 chunk_data = json.loads(line_str)`
- `adapters/runner.py:130 request = json.loads(line_str)`
- `adapters/sglang/client.py:120 chunk_data = json.loads(payload)`
- `adapters/tensorrt/client.py:117 chunk_data = json.loads(payload)`
- `adapters/tgi/client.py:115 chunk_data = json.loads(payload)`
- `adapters/vllm/client.py:126 chunk_data = json.loads(payload)`

### QUA-001 God files over 300 lines (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

Large files may need focused modules.

- `.integrity/mcp-server/server.js 372 lines`
- `adapters/base.py 448 lines`
- `adapters/exllamav2/tests/test_adapter.py 373 lines`
- `adapters/llamacpp/tests/test_adapter.py 368 lines`
- `adapters/ollama/adapter.py 316 lines`
- `adapters/ollama/client.py 301 lines`
- `adapters/runner.py 385 lines`
- `adapters/tests/test_ipc_protocol.py 358 lines`
- `adapters/vllm/adapter.py 332 lines`
- `apps/openbao-trust-demo/main.py 369 lines`
- `compliance-dashboard/app.py 456 lines`
- `compliance-dashboard/tests/test_dashboard.py 447 lines`

### QUA-003 Empty function bodies (HIGH)

Layer: `mapped-check`  
Origin: `john-finding`

Empty bodies and placeholder panics create false confidence.

- `.integrity/mcp-server/server.js:10 * - pre_stage_check: Full verification pass before git add`
- `.integrity/mcp-server/server.js:35 description: "Check a single file for corruption: null bytes, truncation vs HEAD, bracket balance, syntax validity. Returns structured pass/fail report.",`
- `.integrity/mcp-server/server.js:112 return content.includes(0) ? { pass: false, detail: "Null bytes detected" } : { pass: true };`
- `.integrity/mcp-server/server.js:114 return { pass: false, detail: `Read error: ${e.message}` };`
- `.integrity/mcp-server/server.js:125 return { pass: false, detail: `Brace imbalance: { ${open} } ${close} delta ${open - close}` };`
- `.integrity/mcp-server/server.js:128 return { pass: true, warning: `Minor brace mismatch: delta ${open - close}` };`
- `.integrity/mcp-server/server.js:130 return { pass: true };`
- `.integrity/mcp-server/server.js:132 return { pass: false, detail: `Read error: ${e.message}` };`

### QUA-004 Unresolved TODO/FIXME markers (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

TODO/FIXME/HACK/BUG markers indicate unfinished work.

- `docs/ADAPTER-COMPLETION-MATRIX.md:66 | **BUG** | `test_adapter.py:99` asserts `embed()` returns `[0.1, 0.2, 0.3]` but `embed()` returns an `Embedding` dataclass — should assert on `.vector` or `.in`
- `docs/ADAPTER-COMPLETION-MATRIX.md:115 | **BUG** | `test_adapter.py:84–85` asserts `healthy=True` but the adapter returns degraded status when the engine is not ready — the test contradicts the adapt`
- `docs/dougherty/JOHN-REMEDIATION-PLAN.md:54 | 1 | email + QUA-004 | "Extensive TODO placeholders and incomplete implementations throughout" | QUA-004 + Code Smell `Placeholder Implementation` HIGH | Mixed`
- `docs/dougherty/JOHN-REMEDIATION-ROSTER.md:1156 - Adapter stubs: `wc -l` of adapters/*/adapter.py + `grep -c NotImplementedError` = 0 for each. **Important nuance:** John's "extensive TODO placeholders" claim`
- `docs/JOHN-REMEDIATION-PLAN.md:54 | 1 | email + QUA-004 | "Extensive TODO placeholders and incomplete implementations throughout" | QUA-004 + Code Smell `Placeholder Implementation` HIGH | Mixed`
- `docs/JOHN-REMEDIATION-ROSTER.md:1219 - Adapter stubs: `wc -l` of adapters/*/adapter.py + `grep -c NotImplementedError` = 0 for each. **Important nuance:** John's "extensive TODO placeholders" claim`
- `docs/KNOWN-ISSUES.md:126 | `TODO` | 3 files (`mai-adapters/src/manager.rs:586`, `mai-core/src/models/usb.rs:161`, `mai-scheduler/src/default.rs:394/399/402`) | Three carry session-pinne`
- `docs/KNOWN-ISSUES.md:127 | `FIXME`, `unimplemented!` | 0 in src | clean. |`
- `docs/RC1-TESTER-FEEDBACK.md:324 > Extensive use of TODO placeholders and incomplete implementations`
- `docs/SHIP-HARDENING-PLAN.md:1458 TODO`
- `docs/SHIP-HARDENING-PLAN.md:1459 FIXME`
- `mai-adapters/src/manager.rs:586 // TODO: Track in-flight request count per adapter.`

### QUA-007 Mixed async patterns (LOW)

Layer: `mapped-check`  
Origin: `john-finding`

Mixing .then and async/await can reduce maintainability.

- `mai-core/src/models/lifecycle.rs mixes async/await and .then()`

### QUA-008 Modules with 15+ exports (LOW)

Layer: `mapped-check`  
Origin: `john-finding`

Many exports can indicate an unfocused module.

- `mai-adapters/src/bridge.rs 27 exports`
- `mai-agent/src/context.rs 19 exports`
- `mai-agent/src/rag.rs 15 exports`
- `mai-agent/src/stt.rs 15 exports`
- `mai-agent/src/tasks.rs 23 exports`
- `mai-agent/src/tools.rs 22 exports`
- `mai-agent/src/types.rs 39 exports`
- `mai-api/src/auth.rs 22 exports`

### QUA-009 Deeply nested code (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

4+ indentation levels make code harder to read.

- `.integrity/mcp-server/server.js:70 ~4 indentation levels`
- `adapters/base.py:222 ~4 indentation levels`
- `adapters/exllamav2/adapter.py:70 ~4 indentation levels`
- `adapters/exllamav2/client.py:72 ~4 indentation levels`
- `adapters/llamacpp/adapter.py:71 ~4 indentation levels`
- `adapters/llamacpp/client.py:75 ~4 indentation levels`
- `adapters/llamacpp/tests/test_integration_live.py:205 ~4 indentation levels`
- `adapters/ollama/adapter.py:89 ~4 indentation levels`

### QUA-010 .then() without .catch() (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

Promise chains without catch can hide rejections.

- `mai-agent/src/context.rs:532 .then(sa.added_at.cmp(&sb.added_at))`
- `mai-core/src/models/lifecycle.rs:112 backend: summary.capabilities.chat.then(|| "auto".to_string()),`

### CFG-001 Hardcoded localhost URLs in source (LOW)

Layer: `mapped-check`  
Origin: `john-finding`

Hardcoded localhost URLs may not deploy cleanly.

- `.env.example:57 # OLLAMA_HOST=http://127.0.0.1:11434`
- `.env.example:61 # LLAMACPP_HOST=http://127.0.0.1:8081`
- `.github/workflows/ci.yml:126 #       run: curl -s http://localhost:11434/api/tags || (echo "Ollama not available" && exit 1)`
- `.github/workflows/gpu-release.yml:122 if ! curl -fsS http://localhost:11434/api/tags >/dev/null 2>&1; then`
- `adapters/llamacpp/tests/test_integration_live.py:13 export LLAMACPP_HOST=http://127.0.0.1:8081`
- `adapters/llamacpp/tests/test_integration_live.py:88 "set LLAMACPP_HOST=http://127.0.0.1:8081 to enable live tests.",`
- `adapters/ollama/tests/test_adapter.py:52 assert config.base_url == "http://127.0.0.1:11434"`
- `adapters/ollama/tests/test_integration_live.py:9 export OLLAMA_HOST=http://127.0.0.1:11434`

### CFG-002 Unpinned Docker base images (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

Docker images should pin tags or digests.

- `Dockerfile:58 FROM rust:1.88-slim-bookworm AS rust-builder`
- `Dockerfile:121 FROM python:3.12-slim-bookworm AS python-builder`
- `Dockerfile:142 FROM gcr.io/distroless/cc-debian12:nonroot AS runtime`

### TST-004 Test files without assertions (HIGH)

Layer: `mapped-check`  
Origin: `john-finding`

Tests should assert outcomes.

- `adapters/tests/_streaming_server.py no assertion signal`
- `tests/benchmarks/bench_compare.py no assertion signal`

### TST-006 Mock-everything antipattern (MEDIUM)

Layer: `mapped-check`  
Origin: `john-finding`

Excessive mocks may test mocks rather than behavior.

- `adapters/exllamav2/tests/test_adapter.py 27 mock signals`
- `adapters/llamacpp/tests/test_adapter.py 27 mock signals`
- `adapters/ollama/tests/test_adapter.py 31 mock signals`
- `adapters/sglang/tests/test_adapter.py 10 mock signals`
- `adapters/tensorrt/tests/test_adapter.py 9 mock signals`
- `adapters/vllm/tests/test_adapter.py 11 mock signals`

### REV-001 Documented surface with placeholder body (HIGH)

Layer: `mapped-check`  
Origin: `review-integrity`

Docstrings and API comments should not mask unimplemented behavior.

- `mai-sdk-rs/src/lib.rs:768 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:782 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:787 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:792 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:807 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:812 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:819 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:824 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:829 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:836 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:851 documented item ends in placeholder macro`
- `mai-sdk-rs/src/lib.rs:882 documented item ends in placeholder macro`

### REV-002 Adapter/client placeholder density (HIGH)

Layer: `mapped-check`  
Origin: `review-integrity`

Backend integration surfaces should not retain multiple stub or placeholder signals.

- `mai-sdk-python/src/mai/client.py 2 placeholder signals in adapter/backend/client surface`

### REV-003 Polished completion claims beside placeholders (MEDIUM)

Layer: `mapped-check`  
Origin: `review-integrity`

Completion/security claims need implementation evidence when placeholders remain nearby.

- `compliance-dashboard/tests/test_dashboard.py contains completion/security claims and placeholder language`
- `docs/acquisition/ARCHITECTURE.md contains completion/security claims and placeholder language`
- `docs/ADAPTER-COMPLETION-MATRIX.md contains completion/security claims and placeholder language`
- `docs/COGENT-DEPLOYMENT-ROADMAP.md contains completion/security claims and placeholder language`
- `docs/dougherty/JOHN-REMEDIATION-PLAN.md contains completion/security claims and placeholder language`
- `docs/dougherty/JOHN-REMEDIATION-ROSTER.md contains completion/security claims and placeholder language`
- `docs/HANDOFF.md contains completion/security claims and placeholder language`
- `docs/HUMAN-TOUCH-AUDIT.md contains completion/security claims and placeholder language`
- `docs/INDEX.md contains completion/security claims and placeholder language`
- `docs/JOHN-REMEDIATION-PLAN.md contains completion/security claims and placeholder language`
- `docs/JOHN-REMEDIATION-ROSTER.md contains completion/security claims and placeholder language`
- `docs/KNOWN-ISSUES.md contains completion/security claims and placeholder language`

### REV-005 Silent broad error handling (HIGH)

Layer: `mapped-check`  
Origin: `review-integrity`

Broad errors that pass, return None, or default silently hide broken paths.

- `mai-adapters/src/config.rs:99 .unwrap_or_default();`
- `mai-adapters/src/config.rs:252 let content = std::fs::read_to_string(check_file).unwrap_or_default();`
- `mai-agent/src/context.rs:666 .unwrap_or_default()`
- `mai-agent/src/rag.rs:377 .unwrap_or_default()`
- `mai-agent/src/rag.rs:389 .unwrap_or_default()`
- `mai-agent/src/tasks.rs:112 available_tools: request.available_tools.unwrap_or_default(),`
- `mai-agent/src/tools.rs:436 .unwrap_or_default()`
- `mai-api/src/grpc/audit.rs:104 model: e.model_name.clone().unwrap_or_default(),`
- `mai-api/src/grpc/inference.rs:113 .unwrap_or_default()`
- `mai-api/src/grpc/inference.rs:236 .unwrap_or_default()`
- `mai-api/src/handlers/compliance.rs:394 entry: serde_json::to_value(&row.entry).unwrap_or_default(),`
- `mai-api/src/handlers/inference.rs:135 .unwrap_or_default()`

### REV-006 Thin smoke assertions (MEDIUM)

Layer: `mapped-check`  
Origin: `review-integrity`

Assertions should validate outcomes rather than merely confirming execution.

- `apps/compliance-routed/tests/test_integration.py:21 assert spec is not None`
- `apps/compliance-routed/tests/test_smoke.py:19 assert spec is not None`
- `apps/local-secure-inference/tests/test_integration.py:22 assert spec is not None`
- `apps/local-secure-inference/tests/test_smoke.py:24 assert spec is not None`
- `apps/openbao-trust-demo/tests/test_integration.py:23 assert spec is not None`
- `apps/openbao-trust-demo/tests/test_smoke.py:23 assert spec is not None`
- `apps/operator/tests/test_integration.py:22 assert spec is not None`
- `apps/operator/tests/test_smoke.py:22 assert spec is not None`
- `apps/rag-reference/tests/test_integration.py:22 assert spec is not None`
- `apps/rag-reference/tests/test_smoke.py:23 assert spec is not None`
- `apps/tribal-sovereignty/tests/test_integration.py:22 assert spec is not None`
- `apps/tribal-sovereignty/tests/test_smoke.py:19 assert spec is not None`

### REV-007 Duplicated boilerplate blocks (LOW)

Layer: `mapped-check`  
Origin: `review-integrity`

Repeated blocks across modules suggest copy-forward implementation that deserves review.

- `adapters/exllamav2/adapter.py:9 duplicates 6-line block in adapters/llamacpp/adapter.py:10`
- `adapters/exllamav2/adapter.py:10 duplicates 6-line block in adapters/llamacpp/adapter.py:11`
- `adapters/exllamav2/adapter.py:11 duplicates 6-line block in adapters/llamacpp/adapter.py:12`
- `adapters/exllamav2/adapter.py:82 duplicates 6-line block in adapters/llamacpp/adapter.py:81`
- `adapters/exllamav2/adapter.py:92 duplicates 6-line block in adapters/llamacpp/adapter.py:91`
- `adapters/exllamav2/adapter.py:93 duplicates 6-line block in adapters/llamacpp/adapter.py:92`
- `adapters/exllamav2/adapter.py:163 duplicates 6-line block in adapters/llamacpp/adapter.py:98`
- `adapters/exllamav2/adapter.py:164 duplicates 6-line block in adapters/llamacpp/adapter.py:99`

### PRJ-004 Missing dependency lock file (HIGH)

Layer: `mapped-check`  
Origin: `john-finding`

Lock files make installs deterministic.

- `Missing tracked lock file(s): Cargo.lock`
