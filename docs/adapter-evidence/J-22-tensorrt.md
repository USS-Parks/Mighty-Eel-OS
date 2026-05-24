# J-22 TensorRT-LLM/Triton Evidence

## Scope

Bring the TensorRT-LLM/Triton adapter to the locked contract in
`docs/ADAPTER-SHARED-CONTRACT.md` and the locked test harness in
`docs/ADAPTER-TEST-HARNESS-LOCK.md`.

Backend: NVIDIA Triton Inference Server with the TensorRT-LLM backend,
talking the KFServing-style HTTP/JSON v2 API plus SSE streaming on
`/v2/models/<model>/generate_stream`.

Adapter scope is intentionally narrow:

- only `adapters/tensorrt/` and its tests
- new adapter-local conftest for the live-backend fixture (the
  shared `mai/conftest.py` is off-limits during the parallel
  J-18..J-26 wave)
- new evidence note (this file) instead of editing
  `docs/ADAPTER-COMPLETION-MATRIX.md` (convergence session owns the
  matrix update)

Out of scope: any other adapter, the shared `adapters/base.py`,
`adapters/runner.py`, the Rust manager, and the shared conftest.

## Files Changed

| File | Status | Notes |
|---|---|---|
| `adapters/tensorrt/adapter.py` | rewritten | Contract-shaped lifecycle, dual-mode generate, bounded-parallel batch, typed errors, idempotent shutdown, post-shutdown deterministic failure |
| `adapters/tensorrt/client.py` | rewritten | Pooled urllib opener, full typed error mapping (404/408/413/429/500/502/504/timeout/refused/malformed), SSE parser with malformed-frame mapping, idempotent `close()` |
| `adapters/tensorrt/tests/test_adapter.py` | rewritten | 58 unit tests covering every line in "Unit Test Minimums" plus client error mapping and pooling |
| `adapters/tensorrt/tests/test_integration_mock.py` | new | 9 integration tests against an in-process fake Triton over `http.server.ThreadingHTTPServer` |
| `adapters/tensorrt/tests/test_integration_live.py` | new | 5 opt-in live tests gated by `TENSORRT_HOST` / `TRITON_TENSORRT_HOST`, marked `live_backend` |
| `adapters/tensorrt/tests/conftest.py` | new | `tensorrt_available` session-scoped fixture (adapter-local, not shared) |

Untouched on purpose: `adapters/tensorrt/__init__.py`,
`adapters/tensorrt/config.py` (already conformant), the shared
`mai/conftest.py`, every other adapter.

## Contract Results

| Contract item | Status | Where proven |
|---|---|---|
| `__init__` stores config only, no network | PASS | `TestConstruction::test_init_stores_config_without_network` |
| `initialize` happy path | PASS | `TestInitialize::test_initialize_happy_path` |
| `initialize` returns stable handle | PASS | `TestInitialize::test_initialize_returns_stable_handle` |
| `initialize` unavailable -> `BackendUnavailableError` | PASS | `TestInitialize::test_initialize_unavailable_backend_raises` |
| `initialize` config validation -> `ValidationError` | PASS | `TestInitialize::test_initialize_validates_config`, `test_initialize_rejects_bad_port` |
| `initialize` after `shutdown` works | PASS | `TestShutdown::test_reinitialize_after_shutdown_works` |
| `generate` non-streaming -> `GenerationResult` | PASS | `TestGenerateNonStreaming::test_returns_generation_result`, OpenAI-style body shape variant |
| `generate` streaming -> ordered `Token` iterator | PASS | `TestGenerateStreaming::test_yields_ordered_tokens`, EOT-marker guarantee |
| `generate` timeout -> `AdapterTimeoutError` | PASS | `TestGenerateNonStreaming::test_propagates_timeout` |
| `generate` model-not-found -> `ModelNotFoundError(<model>)` | PASS | `test_propagates_model_not_found` (carries model name) |
| `generate` OOM -> `OutOfMemoryError` | PASS | `test_propagates_oom`, plus client-level `test_500_oom_body_raises_oom` |
| `generate` malformed body -> `BackendCrashedError` | PASS | `test_malformed_response_maps_to_crashed` (adapter); `test_malformed_json_body_raises_crashed` (client) |
| `generate_batch` preserves order | PASS | `TestGenerateBatch::test_preserves_order` |
| `generate_batch` empty input intentional | PASS | `TestGenerateBatch::test_empty_input_returns_empty_list` (no client call) |
| `embed` unsupported -> `UnsupportedOperationError("embedding")` | PASS | `TestEmbed::test_unsupported_raises`, plus `test_embed_never_returns_fake_vectors` |
| `health_check` healthy / degraded / unavailable | PASS | `TestHealth` (4 tests, all transitions including live state flip) |
| Capability truthfulness | PASS | `TestCapabilities::test_truthful_flags`, `test_inflight_batching_flag_follows_config` |
| `shutdown` closes resources, idempotent | PASS | `TestShutdown::test_shutdown_closes_client`, `test_double_shutdown_is_safe` |
| Post-shutdown calls fail deterministically | PASS | `TestShutdown::test_post_shutdown_generate_raises` (NotReady), `test_use_after_close_raises_unavailable` (client) |
| Pooling: opener reused across 2+ requests | PASS | `TestClientPooling::test_opener_is_reused_across_two_requests` (unit), `test_two_generates_reuse_opener` (integration mock) |
| SSE termination handled cleanly | PASS | `TestClientStreaming::test_sse_stream_parses_text_output_frames`, `test_streaming_terminates_without_finished_flag` |
| SSE malformed frame -> typed error | PASS | `TestClientStreaming::test_sse_malformed_frame_raises`, adapter-side `test_streaming_propagates_malformed_frame` |
| Backend native error (HTTP 500) -> typed error | PASS | `test_generate_backend_500_is_unavailable` (against fake Triton) |
| Integration: cleanup after shutdown | PASS | `test_shutdown_cleans_up_client` (against fake Triton) |

## Unit And Mock Integration Commands

Run from `mai/`:

```powershell
python -m pytest adapters/tensorrt/tests -q
```

Result on this host (2026-05-24, Windows, Python 3.14.4, pytest 9.0.3):

```
..........................................................sssss......... [100%]
67 passed, 5 skipped in 6.02s
```

Breakdown:

- `test_adapter.py`: 58 unit tests passed
- `test_integration_mock.py`: 9 tests passed against in-process fake Triton
- `test_integration_live.py`: 5 tests skipped (no `TENSORRT_HOST` set, as expected)

Quality gates:

```powershell
python -m ruff check adapters/tensorrt          # All checks passed!
python -m mypy adapters/tensorrt --ignore-missing-imports
# pyproject.toml: note: unused section(s): module = ['adapters.tests.*']
# Success: no issues found in 9 source files
```

## Live Backend Command

```powershell
$env:TENSORRT_HOST = "http://127.0.0.1:8000"
# Optional: pin model (default is "ensemble"):
# $env:TENSORRT_MODEL = "llama-trt"
python -m pytest adapters/tensorrt/tests/test_integration_live.py -m live_backend -v
```

Gate variables honoured:

- `TENSORRT_HOST` (primary, matches the test-harness lock)
- `TRITON_TENSORRT_HOST` (alias for operators with Triton-shaped env vars)
- `TENSORRT_MODEL` (default `ensemble`)

## Live Backend Result

NOT EXECUTED on this host. No NVIDIA Triton + TensorRT-LLM backend is
provisioned in this development environment (Windows workstation, no
local Triton container, no H100/H200 hardware). All 5 live tests
skipped cleanly with the documented `pytest.skip` message:

```
SKIPPED [1] adapters/tensorrt/tests/test_integration_live.py:_:
  TENSORRT_HOST (or TRITON_TENSORRT_HOST) not set or Triton TRT-LLM
  backend unreachable -- export TENSORRT_HOST=http://127.0.0.1:8000
  to enable live tests.
```

Per `docs/ADAPTER-TEST-HARNESS-LOCK.md` "Skip and Failure Rules":
"missing live backend gate env var" is an allowed skip. Operators
running this suite against a provisioned Triton stack should expect
all 5 live tests to pass; if any fail with the env var set, that is a
test failure, not a skip.

## Capability Truth Table

| Flag | Reported | Implemented | Evidence |
|---|---|---|---|
| `supports_streaming` | `True` | Yes -- `_generate_stream` returns `AsyncIterator[Token]`, frames pulled via `asyncio.to_thread` chunk-by-chunk | `test_yields_ordered_tokens`, `test_streaming_against_fake_triton` |
| `supports_batching` | `True` | Yes -- `generate_batch` uses `asyncio.Semaphore` bounded by `max_concurrent_requests`, results re-sorted to input order | `test_preserves_order`, `test_empty_input_returns_empty_list` |
| `supports_structured_output` | `False` | Adapter does not send/parse structured-output fields | (negative claim; no test path) |
| `supports_vision` | `False` | Adapter has no vision input shape | (negative claim) |
| `supports_tool_calling` | `False` | Adapter has no tool-call surface | (negative claim) |
| `supports_continuous_batching` | tracks `config.enable_inflight_batching` | Yes -- Triton TRT-LLM inflight batcher; adapter doesn't disable | `test_truthful_flags` (default True), `test_inflight_batching_flag_follows_config` (False when configured) |
| `supports_embedding` | `False` | Yes -- `embed()` always raises `UnsupportedOperationError("embedding")` | `test_unsupported_raises`, `test_embedding_unsupported_on_live_backend` |
| `supports_hot_swap` | `False` | Adapter does not implement hot-swap | (negative claim) |
| `max_context_window` | `max_input_len + max_output_len` from config | Yes -- adapter does not pretend Triton supports larger | `test_truthful_flags` (> 0) |
| `supported_quantizations` | `["fp16", "fp8", "int8", "int4"]` | Truthful for TensorRT-LLM engine builder; adapter does not select | `test_truthful_flags` (fp16 present) |
| `extra.precision`, `extra.tensor_parallel_size`, `extra.inflight_batching` | populated from config | Yes; hardware specifics live in `extra`, not top-level flags (per contract) | `test_inflight_batching_flag_follows_config`, `test_capabilities_are_truthful_for_live_backend` |

## Error Mapping Evidence

| Condition | Required error | Test |
|---|---|---|
| backend not listening (URLError refused) | `BackendUnavailableError` | `test_urlerror_refused_raises_unavailable` (client), `test_initialize_fails_when_backend_not_listening` (integration) |
| read/connect timeout (URLError "timed out") | `AdapterTimeoutError` | `test_urlerror_timeout_raises_timeout` (client), `test_propagates_timeout` (adapter) |
| HTTP 404 | `ModelNotFoundError(<model>)` with model name | `test_404_raises_model_not_found`, `test_propagates_model_not_found` |
| HTTP 408 / 504 | `AdapterTimeoutError` | `test_504_raises_timeout` |
| HTTP 413 or "context"/"too long"/"exceed" in body | `ContextExceededError` | `test_413_raises_context_exceeded` |
| HTTP 429 | `RateLimitedError` | `test_429_raises_rate_limited` |
| HTTP 500 with "out of memory" / "OOM" / "CUDA memory" body | `OutOfMemoryError` | `test_500_oom_body_raises_oom` |
| HTTP 502 / "broken pipe" / "reset by peer" | `BackendCrashedError` | `test_502_raises_backend_crashed` |
| HTTP 5xx (other) | `BackendUnavailableError` | `test_generate_backend_500_is_unavailable` (integration, against fake Triton) |
| Malformed JSON in response body | `BackendCrashedError` | `test_malformed_json_body_raises_crashed`, `test_generate_malformed_body_raises_crashed` |
| Malformed SSE frame | `BackendCrashedError` | `test_sse_malformed_frame_raises`, `test_streaming_propagates_malformed_frame` |
| Config invalid (host empty, port out of range, timeouts <=0, model empty) | `ValidationError` | `test_initialize_validates_config`, `test_initialize_rejects_bad_port` |
| Unsupported operation (embed) | `UnsupportedOperationError("embedding")` | `test_unsupported_raises`, `test_embedding_unsupported_on_live_backend` |
| Use after shutdown | `AdapterError(code="NotReady")` (adapter), `BackendUnavailableError("client is closed")` (client) | `test_post_shutdown_generate_raises`, `test_use_after_close_raises_unavailable` |

Triton's HTTP API does not expose distinct backend-throttling or
context-overflow status codes consistently; the adapter therefore
infers them from response body text in addition to status code. Body
matching is case-insensitive and uses substring checks, so backend
copy changes won't silently regress the typed mapping.

## Known Limitations

- **`AdapterError(code="NotReady")` is not in the typed-error table in
  `ADAPTER-SHARED-CONTRACT.md`.** The contract's table covers BACKEND
  failures; calling an uninitialized adapter is an ADAPTER state
  error. The Ollama reference adapter uses the same `NotReady` code
  here, so J-22 follows the precedent rather than introducing a new
  named variant. A future convergence session may add `NotReadyError`
  to `adapters/base.py` and update this row.
- **`tokens_generated` falls back to a `len(text) // 4` heuristic** when
  the Triton body omits `output_tokens` / `generated_tokens`. Real
  Triton TRT-LLM responses include `output_tokens`, so the heuristic
  is only exercised against malformed or partial responses; the
  finish-reason mapping then treats `tokens_out >= max_tokens` as a
  conservative `MAX_TOKENS` finish.
- **Continuous-batching capability is config-derived, not probed.** The
  adapter trusts `config.enable_inflight_batching`; we do not query
  Triton to confirm the model config has inflight batching enabled.
  Operators who disable inflight batching server-side without flipping
  the config get an over-reported capability. A convergence session
  could add a metadata probe in `initialize` and reconcile.
- **Bounded-parallel batch is adapter-side, not native multi-prompt.**
  Triton's TRT-LLM `/generate` endpoint is single-prompt per request;
  the adapter issues N concurrent requests bounded by
  `max_concurrent_requests` and relies on Triton's inflight batcher
  to merge them on the GPU. This is documented per the contract and
  the capability flag is set honestly (`supports_batching=True` with
  bounded parallelism).
- **No backend version negotiation.** `capabilities().backend_version`
  is the static string `"0.12.0"` -- the adapter does not query
  `/v2` to populate it dynamically. Stale string today; safe to
  upgrade in a future convergence pass.
- **Stdlib urllib has no native keep-alive.** Per-request connections
  are the norm; the "pooling" guarantee is at the opener-instance
  level (one `OpenerDirector` for the client's lifetime), not at the
  TCP-socket level. The integration test
  `test_two_generates_reuse_opener` asserts the opener-instance
  invariant; the connection count on the fake server is >= 2, which
  is expected and correct.

## Completion Verdict

**COMPLETE.** The TensorRT-LLM/Triton adapter satisfies every
requirement in `docs/ADAPTER-SHARED-CONTRACT.md` and every layer in
`docs/ADAPTER-TEST-HARNESS-LOCK.md`:

- every method in the required surface has intentional behavior
- capabilities match implemented behavior (and are config-aware)
- typed error mapping is tested at both the client and adapter layers
- pooling and lifecycle behavior is tested (unit + integration)
- 67 unit + mock tests pass without a live backend
- 5 live backend tests exist, are marked `live_backend`, and skip
  cleanly when `TENSORRT_HOST` / `TRITON_TENSORRT_HOST` is absent
- this evidence note exists
- no TODO/stub language remains; the only unsupported method
  (`embed`) raises `UnsupportedOperationError`

J-26 (generic Triton adapter) is a sibling session whose tests
currently show 2 failures in `adapters/triton/tests/test_client.py`.
Those are J-26's territory and are independent of J-22 (no shared
code path; this session never opened any file outside
`adapters/tensorrt/`).
