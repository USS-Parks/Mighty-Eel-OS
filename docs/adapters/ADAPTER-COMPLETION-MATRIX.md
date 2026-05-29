# Adapter Completion Matrix

**Owner:** DOUGHERTY lane J-05 (`docs/dougherty/JOHN-REMEDIATION-PLAN.md` §3 W3)
**Scope:** the seven adapters already in `mai/adapters/` plus four expansion adapters that close out W3.
**Authoritative as of:** 2026-05-24, HEAD `e32d8fe`
**Audit method:** parallel `Explore`-agent profile of every `adapter.py` / `client.py` / `config.py` / `tests/test_adapter.py` under `mai/adapters/`. Bug discoveries inline.

This document is the input for J-06, J-07, and J-18..J-26. Read this BEFORE opening any of those sessions.

---

## 1. Common findings (all seven existing adapters)

These observations apply to every backend; documenting once here keeps the per-adapter sections short.

| Concern | Status | Evidence | Fixed by |
|:--|:--|:--|:--|
| HTTP transport | All seven use stdlib `urllib.request`, ZERO `httpx.AsyncClient` | every `client.py` | by design (air-gap, no third-party HTTP dep in the core) |
| Client-instance reuse | One `*Client` instantiated in `initialize()` and stored on `self._client`; reused for every subsequent request | adapter.py per backend | already correct |
| Connection pooling (TCP) | Implicit via OS socket cache; no explicit `urllib3.PoolManager` or persistent session | all `client.py` | N/A — `urllib` doesn't expose pooling. Acceptable for localhost-only air-gap traffic; revisit only if profiling shows per-request connect dominates |
| Lifecycle | Every `shutdown()` nulls `self._client = None`; no `__aenter__`/`__aexit__` on `AdapterBase` | adapter.py:shutdown per backend | **J-12** (async context managers) |
| Embed stub coverage | 5 of 7 raise `UnsupportedOperationError` for `embed()`; only Ollama and vLLM implement it | per-backend `embed()` | by design (most backends don't serve embeddings); honest capability flag exposure required |
| Error mapping consistency | Backend-unavailable + timeout: handled everywhere. Context-exceeded + rate-limit + malformed-response: inconsistent — some adapters handle, others don't | client.py per backend | **J-08** (error path audit) is the consistency pass |
| Live-backend integration tests | All seven use `AsyncMock`; ZERO opt-in live tests today | every `tests/test_adapter.py` | **J-06** (Ollama), **J-07** (llama.cpp), **J-18..J-22** (others) |
| Test-assertion bugs | Three real bugs surfaced during this audit — see §3 below | inline | each tied to its J-session |

**Pooling decision (recorded here so the J-08 audit and J-18..J-26 sessions don't relitigate):** the urllib-based one-client-per-adapter-instance pattern stays. Adding `httpx` or `aiohttp` would pull a third-party HTTP stack into the air-gapped core, which contradicts the threat model documented in `ARCHITECTURE.md` and the DOUGHERTY plan §8 "stdlib-only" carve-out. Connection pooling becomes a real concern only if profiling under load shows per-request connect overhead dominating latency.

---

## 2. Existing adapters

### 2.1 Ollama  (`adapters/ollama/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 316 / client.py 293 / config.py 68 / test_adapter.py 274 |
| Methods | initialize ✓ (adapter.py:60–106) · generate ✓ (108–138) · stream ✓ (implicit via `_collect_stream`) · embed ✓ (207–227) · health ✓ (229–242) · capabilities ✓ (244–258) · shutdown ✓ (260–264) |
| Capability flags | streaming=T, batching=F, embedding=T, vision=F, tool_calling=F, hot_swap=F, structured_output=F |
| Error mapping | backend-unavailable ✓ · model-missing ✓ (404→ModelNotFoundError, client.py:287) · timeout ✓ · context-exceeded ✗ · malformed-response ✗ · rate-limit ✗ · backend-crash ✓ (500+OOM, client.py:288–289) |
| Tests | 22 assertions (per audit), all mocked, no live tests |
| Verdict | **COMPLETE** for the core inference path; closes out under J-06 once live tests land. |
| J-session | **J-06** |

### 2.2 llama.cpp  (`adapters/llamacpp/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 273 / client.py 247 / config.py 76 / test_adapter.py 83 |
| Methods | initialize ✓ (57–94) · generate ✓ (101–142) · stream ✓ (144–179) · embed STUB (216–218, UnsupportedOperationError) · health ✓ (220–233) · capabilities ✓ (235–251) · shutdown ✓ (253–257) |
| Capability flags | streaming=T, batching=F, structured_output=T (GBNF), embedding=F, vision=F, tool_calling=F |
| Error mapping | backend-unavailable ✓ · timeout ✓ · model-missing ✓ · context-exceeded ✓ (client.py:154–155) · OOM ✓ (client.py:151–152) · rate-limit ✗ · malformed-response ✗ (silently continues at client.py:121) · backend-crash partial (500→BackendUnavailableError) |
| Tests | 7-11 assertions, all mocked, no live tests; streaming, batching, grammar paths untested |
| Verdict | **NEEDS-FIX** — core paths implemented but test surface is thin AND there is a capability-flag naming inconsistency: `capabilities()` exposes `supports_embedding=False` (singular) but the test asserts the wrong name. Both addressed in J-07 and J-09. |
| J-session | **J-07** (live tests) and **J-09** (assertion fill) |

### 2.3 vLLM  (`adapters/vllm/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 332 / client.py 257 / config.py 78 / test_adapter.py 99 |
| Methods | initialize ✓ (55–104) · generate ✓ (111–158, with `guided_json` structured output) · stream ✓ (160–194, SSE) · embed ✓ (234–256) · health ✓ (258–267) · capabilities ✓ (269–285) · shutdown ✓ (287–291) |
| Capability flags | streaming=T, batching=T, structured_output=T, vision=F, tool_calling=T, continuous_batching=T, embedding=T, hot_swap=T (LoRA) |
| Error mapping | most complete of the 7: backend-unavailable ✓ · model-missing ✓ · timeout ✓ · context-exceeded ✓ (client.py:162–163, parses "context length"/"too long") · rate-limit ✓ (429→RateLimitedError, client.py:149–150) · malformed-response partial (caught + continues, client.py:126–128) · backend-crash maps to unavailable (no distinct type) |
| Tests | 16 assertions, all mocked, no live tests; generate_batch / streaming / LoRA hot-swap / error paths all untested |
| **BUG** | `test_adapter.py:99` asserts `embed()` returns `[0.1, 0.2, 0.3]` but `embed()` returns an `Embedding` dataclass — should assert on `.vector` or `.input_tokens`. Fix in **J-18**. |
| Verdict | **COMPLETE** for adapter contract; the test-assertion bug needs fixing alongside the J-18 live-test work. |
| J-session | **J-18** (live tests + embed assertion bug fix) |

### 2.4 TGI  (`adapters/tgi/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 233 / client.py 198 / config.py 67 / test_adapter.py 82 |
| Methods | initialize ✓ (57–93) · generate ✓ (99–135) · stream ✓ (137–164, SSE) · embed STUB (198–200) · health ✓ (202–211) · capabilities ✓ (213–227) · shutdown ✓ (229–233) |
| Capability flags | streaming=T, batching=T, structured_output=F, vision=F, tool_calling=F, continuous_batching=T, embedding=F, hot_swap=F |
| Error mapping | backend-unavailable ✓ · timeout ✓ · rate-limit ✓ (client.py:132) · context-exceeded ✓ (client.py:144) · OOM ✓ (client.py:142) · model-missing ✗ · malformed-response partial (silent JSON skip at client.py:116) |
| Tests | 14 assertions, all mocked, no live tests; batching / streaming / watermarking / quantization untested |
| Verdict | **COMPLETE** for contract; live tests + model-missing handler land under J-19. |
| J-session | **J-19** |

### 2.5 SGLang  (`adapters/sglang/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 249 / client.py 244 / config.py 67 / test_adapter.py 101 |
| Methods | initialize ✓ (41–66) · generate ✓ (68–123, supports constrained decoding) · stream ✓ (125–143) · embed STUB (157–161) · health ✓ (163–172) · capabilities ✓ (174–189) · shutdown ✓ (191–193) |
| Capability flags | streaming=T, batching=T, embedding=F, tool_calling=T, structured_output=T, radix_attention=T, constrained_decoding=T, fork_parallelism=T |
| Error mapping | most complete after vLLM: backend-unavailable ✓ · timeout ✓ · model-missing ✓ · rate-limit ✓ · context-exceeded ✓ · backend-crash ✓ · OOM ✓ · malformed-response ✗ |
| Tests | 18 assertions, all mocked, no live tests; generate_batch / shutdown / health-when-healthy / constrained-decoding-regex / malformed JSON paths untested |
| Verdict | **COMPLETE** for contract; live tests land under J-20. |
| J-session | **J-20** |

### 2.6 ExLlamaV2  (`adapters/exllamav2/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 289 / client.py 199 / config.py 68 / test_adapter.py 81 |
| Methods | initialize ✓ (56–96) · generate ✓ (102–144) · stream ✓ (146–175) · embed STUB (214–216) · health ✓ (218–227) · capabilities ✓ (229–244) · shutdown ✓ (246–250) |
| Capability flags | streaming=T, batching=T, structured_output=F, vision=F, tool_calling=F, continuous_batching=F, embedding=F, hot_swap=T |
| Error mapping | backend-unavailable ✓ · model-missing ✓ · timeout ✓ · context-exceeded ✗ · malformed-response partial · rate-limit ✗ · backend-crash maps to unavailable |
| Tests | 9 assertions, all mocked, no live tests; streaming / batch / model load-unload-switch / health untested |
| Verdict | **COMPLETE** for contract but tied with TensorRT-LLM for thinnest tests (9). Live tests + assertion fill under J-21 and J-09. |
| J-session | **J-21** (live tests) and **J-09** (assertion fill) |

### 2.7 TensorRT-LLM/Triton  (`adapters/tensorrt/`)

| Field | Value |
|:--|:--|
| Files | adapter.py 253 / client.py 209 / config.py 71 / test_adapter.py 85 |
| Methods | initialize ✓ (56) · generate ✓ (101) · stream ✓ via `_generate_stream` (140) · embed STUB (199) · health_check ✓ (203) · capabilities ✓ (216) · shutdown ✓ (233) |
| Capability flags | streaming=T, batching=T, structured_output=F, vision=F, tool_calling=F, continuous_batching=T, embedding=F, hot_swap=F |
| Error mapping | backend-unavailable ✓ · model-missing ✓ · timeout ✓ · context-exceeded ✗ · malformed-response partial (silent JSON skip in stream, client.py:118) · rate-limit ✗ · backend-crash maps to unavailable |
| Tests | 14 assertions, all mocked, no live tests; streaming / batch / shutdown / `is_engine_ready` / `get_model_metadata` untested |
| **BUG** | `test_adapter.py:84–85` asserts `healthy=True` but the adapter returns degraded status when the engine is not ready — the test contradicts the adapter logic. Fix in **J-22**. |
| Verdict | **NEEDS-FIX** — health-check test bug is a real assertion failure waiting to happen on first real run, and the thin coverage (no streaming, no batch) means deployment risk. |
| J-session | **J-22** (live tests + health-check bug fix) |

---

## 3. Bugs surfaced during this audit (must close in the named J-session)

1. **vLLM embed return-type assertion bug** — `adapters/vllm/tests/test_adapter.py:99` asserts the literal list `[0.1, 0.2, 0.3]` against a function that returns an `Embedding` dataclass. The test currently passes only because the mock returns the literal list; against a real backend it will fail. → fix in **J-18**.
2. **TensorRT-LLM health-check assertion contradicts adapter** — `adapters/tensorrt/tests/test_adapter.py:84–85` expects `healthy=True` while the adapter reports degraded when the engine is not ready. → fix in **J-22**.
3. **llama.cpp capability-flag name drift** — adapter exposes `supports_embedding` (singular) while the test asserts `supports_embeddings` (plural). One of them must move; the adapter is the source of truth so the test changes. → fix in **J-09**.

---

## 4. Expansion adapters (J-23..J-26)

Each is a new directory under `mai/adapters/<name>/` following the established pattern: `adapter.py` (subclass of `AdapterBase`) + `client.py` (stdlib `urllib`-based HTTP wrapper or subprocess wrapper) + `config.py` (dataclass) + `tests/test_adapter.py` (≥30 assertions, all mocked).

### 4.1 J-23 — Generic OpenAI-compatible local adapter  (`adapters/openai_compat/`)

**Why it exists.** LM Studio, LocalAI, FastChat, vLLM-via-OpenAI-mode, and most custom internal gateways all speak the OpenAI REST shape (`/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/models`). A single adapter that points at any of them is the highest-leverage W3 addition.

**Minimum public API:** every `AdapterBase` method except `embed` (which is conditional on the backend declaring it via `/v1/models`).

**Config schema:**
```toml
[adapter.openai_compat]
base_url = "http://127.0.0.1:1234/v1"   # backend's /v1 root
api_key = ""                            # optional; some backends require it
default_model = ""                      # leave empty to use the first /v1/models entry
request_timeout_s = 60
```

**Env var:** `OPENAI_COMPAT_HOST` — used by `tests/test_integration_live.py` to opt into live tests against whatever's listening (LM Studio, LocalAI, etc.).

**Unit-test matrix:** initialize-happy + initialize-no-backend + generate + stream + embed-when-supported + embed-when-unsupported (UnsupportedOperationError) + health + capabilities + shutdown-idempotent + auth-required-but-missing + 429-rate-limit + 500-backend-crash. ≥30 assertions.

**Live-test opt-in command:** `OPENAI_COMPAT_HOST=http://127.0.0.1:1234 pytest -m live_backend adapters/openai_compat/tests/test_integration_live.py`

**Acceptance criteria:**
- adapter is registered via `@mai_adapter(name="openai_compat")`
- live test passes against LM Studio with a tinyllama-class model
- capabilities() reflects the backend's actual `/v1/models` response, not hardcoded flags
- pytest-assertion-gate (J-10) clears with ≥30 assertions

### 4.2 J-24 — ONNX Runtime adapter  (`adapters/onnx/`)

**Why it exists.** CPU and DirectML-only Windows deployments need a deterministic path that avoids GPU drivers entirely. ONNX Runtime is the standard for non-NVIDIA enterprise inference; Azure and on-prem Windows shops will look for this adapter explicitly.

**Minimum public API:** initialize, generate (sync only, no streaming in v1), health, capabilities, shutdown. `stream` is implemented as a single-event yield wrapping `generate` until real ONNX Runtime token streaming lands. `embed` only if the loaded model is an embedding model (sentence-transformers ONNX exports).

**Config schema:**
```toml
[adapter.onnx]
model_path = "/opt/mai/models/llama-3.1-8b-onnx"     # absolute path to the .onnx file or its directory
execution_provider = "CPUExecutionProvider"          # or "DmlExecutionProvider" for DirectML
intra_op_num_threads = 0                             # 0 = use all
graph_optimization_level = "all"
```

**Env var:** none for the adapter itself; model location is config-driven (the J-24 live test reads `ONNX_MODEL_PATH` to point at a small ONNX model for CI).

**Unit-test matrix:** initialize-with-missing-model-file + initialize-happy + generate-deterministic-with-seed + stream-yields-single-event + capabilities + health + shutdown + execution-provider-fallback + tensor-shape-mismatch-error + tokenizer-mismatch-error. ≥30 assertions.

**Live-test opt-in command:** `ONNX_MODEL_PATH=./tests/_models/tiny.onnx pytest -m live_backend adapters/onnx/tests/test_integration_live.py`

**Acceptance criteria:**
- works on CPU without a GPU
- ONNX Runtime is the ONLY third-party Python dep added by this session (pinned in pyproject.toml, lock regenerated per J-03 policy)
- DirectML execution-provider path tested at least at the config-validation layer

### 4.3 J-25 — MLX adapter  (`adapters/mlx/`)

**Why it exists.** Apple Silicon dev nodes and secure-edge deployments use MLX as the local inference runtime. Without an adapter, every Apple Silicon developer is forced into the OpenAI-compat adapter (J-23) pointed at an MLX HTTP wrapper, which is two layers of indirection.

**Minimum public API:** initialize, generate, stream, health, capabilities, shutdown. No embed in v1.

**Config schema:**
```toml
[adapter.mlx]
model_id = "mlx-community/Llama-3.2-1B-Instruct-4bit"
max_kv_size = 8192
trust_remote_code = false
```

**Env var:** `MLX_LIVE=1` to opt into the live-backend test (which spawns the MLX runtime in-process — no separate server needed).

**Unit-test matrix:** initialize-with-missing-model + initialize-happy + generate-deterministic-with-seed + stream-yields-tokens + capabilities-reflects-quantization + health + shutdown + hardware-gated-skip-when-not-on-apple-silicon. ≥30 assertions.

**Live-test opt-in command:** `MLX_LIVE=1 pytest -m live_backend adapters/mlx/tests/test_integration_live.py` (auto-skips on non-Apple-Silicon hosts)

**Acceptance criteria:**
- adapter declares `requires_platform = "darwin-arm64"` in capabilities and the integration-test fixture honours it
- live test runs and passes on at least one Apple Silicon machine; skips cleanly elsewhere
- MLX pinned in pyproject.toml as a platform-marker optional dep (`mlx; platform_system == "Darwin" and platform_machine == "arm64"`)

### 4.4 J-26 — Generic Triton adapter  (`adapters/triton/`)

**Why it exists.** TensorRT-LLM (J-22) is one consumer of NVIDIA Triton Inference Server. The OTHER consumers — multimodal models, classifiers, custom Python backends, image embedding models — also speak Triton's HTTP/gRPC protocol and currently have NO adapter. J-26 covers the non-LLM Triton surface so MAI can serve classifier and multimodal workloads.

**Minimum public API:** initialize, generate (treated as `infer` for non-LLM models — input tensor in, output tensor out), health, capabilities, shutdown. Streaming is OPTIONAL and only enabled when the loaded model's config declares decoupled mode. No embed in v1.

**Config schema:**
```toml
[adapter.triton]
base_url = "http://127.0.0.1:8000"
model_name = "ensemble_pipeline"
model_version = "1"                  # or "latest"
protocol = "http"                    # or "grpc"
```

**Env var:** `TRITON_HOST` for the live test (points at a running Triton serving any model).

**Unit-test matrix:** initialize-against-unreachable-triton (BackendUnavailable) + initialize-with-missing-model (ModelNotFoundError) + initialize-happy + infer-with-tensor-input + capabilities-reflects-model-config + health (queries Triton's `/v2/health/ready`) + shutdown + protocol-fallback-http-to-grpc. ≥30 assertions.

**Live-test opt-in command:** `TRITON_HOST=http://127.0.0.1:8000 pytest -m live_backend adapters/triton/tests/test_integration_live.py`

**Acceptance criteria:**
- reuses the Triton HTTP client code from J-22 where possible (DRY across the two Triton-flavoured adapters)
- live test passes against a Triton instance serving a non-LLM model (e.g. a classifier ONNX model loaded into Triton)
- shares its config validation with J-22 so both adapters fail the same way on bad URLs

---

## 5. Summary table

| # | Adapter | Category | Methods status | Pooling | Lifecycle | Live test | Verdict | Fix Session |
|---|:--|:--|:--|:--|:--|:--|:--|:--|
| 1 | Ollama | mandatory existing | all 7 implemented | adapter-instance reuse | manual | none yet | COMPLETE | **J-06** |
| 2 | llama.cpp | mandatory existing | 6 / embed STUB | adapter-instance reuse | manual | none yet | NEEDS-FIX (thin tests + capability typo) | **J-07** + **J-09** |
| 3 | vLLM | mandatory existing | all 7 implemented | adapter-instance reuse | manual | none yet | COMPLETE (embed test bug) | **J-18** |
| 4 | TGI | mandatory existing | 6 / embed STUB | adapter-instance reuse | manual | none yet | COMPLETE | **J-19** |
| 5 | SGLang | mandatory existing | 6 / embed STUB | adapter-instance reuse | manual | none yet | COMPLETE | **J-20** |
| 6 | ExLlamaV2 | mandatory existing | 6 / embed STUB | adapter-instance reuse | manual | none yet | COMPLETE (thin tests) | **J-21** + **J-09** |
| 7 | TensorRT-LLM/Triton | mandatory existing | 6 / embed STUB | adapter-instance reuse | manual | none yet | NEEDS-FIX (health-check test bug) | **J-22** |
| 8 | Generic OpenAI-compat | expansion | NEW | adapter-instance reuse | manual | OPENAI_COMPAT_HOST | NEW-ADAPTER-NEEDED | **J-23** |
| 9 | ONNX Runtime | expansion | NEW | in-process | manual | ONNX_MODEL_PATH | NEW-ADAPTER-NEEDED | **J-24** |
| 10 | MLX | expansion | NEW | in-process | manual | MLX_LIVE (auto-skip non-Apple) | NEW-ADAPTER-NEEDED | **J-25** |
| 11 | Generic Triton | expansion | NEW | adapter-instance reuse | manual | TRITON_HOST | NEW-ADAPTER-NEEDED | **J-26** |

**Closing condition for W3:** every row of this table has a corresponding closed J-session, the bugs in §3 are committed-fixed, and `pytest -m live_backend` is documented for every backend whose live opt-in env var the operator supplies.
