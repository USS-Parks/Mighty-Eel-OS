# J-24 ONNX Runtime Evidence

## Scope

Add a new MAI adapter that wraps **Microsoft ONNX Runtime** for in-process
inference on CPU, DirectML, and CUDA. The adapter is the CPU /
enterprise-Windows fallback path; it does not spawn an external server.

The adapter inherits `AdapterBase` and implements the full required
surface from `docs/ADAPTER-SHARED-CONTRACT.md`. Generation is wrapped
through **onnxruntime-genai** when the loaded model supports
autoregressive decoding; otherwise the adapter degrades to embedding
only and raises `UnsupportedOperationError` for `generate` /
`generate_batch`. Capability flags are determined at `initialize()`
time, not import time, because the same package serves three legitimate
model shapes (genai generation, embedding-only InferenceSession,
unusable encoder-only InferenceSession).

Owned files only (per shared-contract §Ownership Rules):
- `mai/adapters/onnxruntime/` (new package)
- `mai/adapters/onnxruntime/tests/` (adapter-owned tests)
- this evidence note

No shared files touched. Registry wiring uses the existing
`@mai_adapter(name="onnxruntime", version="1.0.0")` decorator + runner
fallback path, so `adapters/runner.py` is unchanged.

## Files Changed

```
A  mai/adapters/onnxruntime/__init__.py                       (15 lines)
A  mai/adapters/onnxruntime/config.py                         (76 lines)
A  mai/adapters/onnxruntime/client.py                        (373 lines)
A  mai/adapters/onnxruntime/adapter.py                       (366 lines)
A  mai/adapters/onnxruntime/tests/__init__.py                  (1 line)
A  mai/adapters/onnxruntime/tests/test_adapter.py            (762 lines)
A  mai/adapters/onnxruntime/tests/test_integration_live.py   (214 lines)
A  mai/docs/adapter-evidence/J-24-onnxruntime.md             (this file)
```

## Contract Results

Required-surface check against `docs/ADAPTER-SHARED-CONTRACT.md`
§Required Python Adapter Surface:

| Method | Intentional behavior | Evidence test |
|---|---|---|
| `initialize` | Loads via worker thread, resolves capability flags from loaded model, returns `f"onnxruntime:{backend}:{model_id}"` handle | `TestAdapterLifecycle::test_initialize_happy_path` |
| `generate` (non-stream) | Returns `GenerationResult` with finish reason; respects `params.max_tokens` and adapter-level timeout | `TestAdapterGenerate::test_generate_non_streaming_returns_result`, `…_max_tokens_finish_reason` |
| `generate` (stream) | Yields ordered `Token` objects with terminator; respects `stream_timeout_ms` | `TestAdapterGenerate::test_generate_streaming_yields_ordered_tokens`, `TestClientIntegrationGenai::test_generate_stream_emits_terminator` |
| `generate_batch` | Sequential — preserves input order; empty input returns empty list | `TestAdapterBatch::test_batch_preserves_order`, `…_empty_input_returns_empty` |
| `embed` | Returns `Embedding` instances; raises `UnsupportedOperationError` when generation-only | `TestAdapterEmbed::test_embed_returns_typed_embeddings`, `…_unsupported` |
| `health_check` | Reports healthy / degraded / unavailable based on `_initialized` + backend readiness | `TestAdapterHealth::*` |
| `capabilities` | Truthful post-initialize snapshot — `supports_streaming` reflects `_supports_generation`; `supports_embedding` reflects `_supports_embedding` | `TestAdapterCapabilities::*` |
| `shutdown` | Closes client, clears `_initialized`, idempotent | `TestAdapterLifecycle::test_shutdown_closes_client`, `…_is_idempotent`, `…_post_shutdown_calls_fail_deterministically` |

Lifecycle contract (`§Lifecycle Contract`):

- `__init__` does no I/O — proven by `test_construction_does_no_io`.
- `initialize` validates path, imports runtime in worker thread, sets
  ready flag — proven by `test_initialize_happy_path`,
  `test_initialize_validation_error_when_path_missing`.
- Pre-init calls fail with `BackendUnavailableError` — proven by
  `test_post_shutdown_calls_fail_deterministically`.
- `shutdown` is idempotent — proven by `test_shutdown_is_idempotent`.
- `__aenter__` / `__aexit__` not added (adapter does not expose them);
  contract clause "if an adapter adds them" therefore vacuously holds.

HTTP / session pooling (`§HTTP And Session Pooling`):

ONNX Runtime is in-process, so the relevant analogue is **InferenceSession
reuse across calls**. Proven by
`TestClientIntegrationMocked::test_embedding_session_load_and_run`,
which asserts the same `_FakeInferenceSession` instance receives two
`run()` calls across two `embed()` invocations on the same client.

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters/onnxruntime/tests -q
```

Result:

```
.............................................ssssss                  [100%]
45 passed, 6 skipped in 0.87s
```

The 6 skipped are the live-backend tests in `test_integration_live.py`
that auto-skip when `ONNXRUNTIME_MODEL_PATH` is unset — exactly the
behavior required by `docs/ADAPTER-TEST-HARNESS-LOCK.md` §Live Backend
Test Minimums ("Skip if the gate env var is absent").

Quality gates (run from `mai/`):

```powershell
python -m ruff check adapters/onnxruntime/        # All checks passed!
python -m mypy adapters/onnxruntime/              # Success: no issues found in 7 source files
```

## Live Backend Command

```powershell
$env:ONNXRUNTIME_MODEL_PATH = "C:\models\phi-3-mini-4k-instruct-onnx"
python -m pytest -m live_backend adapters/onnxruntime/tests/test_integration_live.py -v
```

Optional env vars honored by the live tests:

| Env var | Effect |
|---|---|
| `ONNXRUNTIME_MODEL_PATH` | Required — points at a real .onnx file or a `onnxruntime-genai` model directory. |
| `ONNXRUNTIME_EMBEDDING_ONLY` | Set to `1` to force the embedding-only branch; default `0`. |
| `ONNXRUNTIME_CONTEXT_WINDOW` | Override the reported `max_context_window`; default `4096`. |
| `ONNXRUNTIME_MAX_TOKENS` | Per-test generation budget; default `16`. |
| `ONNXRUNTIME_PROVIDERS` | Comma list, e.g. `"DmlExecutionProvider,CPUExecutionProvider"`. |

## Live Backend Result

**Not exercised in this session.** No ONNX Runtime wheel and no model
weights are installed on the J-24 dev host. The live tests are wired up
and proven to skip cleanly (6 skips in the pytest run above); when the
operator supplies `ONNXRUNTIME_MODEL_PATH` plus a working
`onnxruntime` install, the suite runs without further code changes.

Per `docs/feedback_test_evidence_literalism.md`: implementation is
closed (capability claims + tests + skips); live evidence does not
exist yet.

## Capability Truth Table

| Flag | Generation-mode (genai) | Embedding-only (session) | Encoder/no-genai degraded |
|---|---|---|---|
| `supports_streaming` | `True` | `False` | `False` |
| `supports_batching` | `False` (sequential) | `False` | `False` |
| `supports_structured_output` | `False` | `False` | `False` |
| `supports_vision` | `False` | `False` | `False` |
| `supports_tool_calling` | `False` | `False` | `False` |
| `supports_continuous_batching` | `False` | `False` | `False` |
| `supports_embedding` | `False` | `True` | `False` |
| `supports_hot_swap` | `False` | `False` | `False` |
| `max_context_window` | from `OnnxRuntimeConfig.context_window` (default 4096) | same | same |
| `supported_quantizations` | `["onnx_fp32", "onnx_fp16", "onnx_int8"]` | same | same |
| `backend_version` | `onnxruntime_genai.__version__` | `onnxruntime.__version__` | `onnxruntime.__version__` |
| `extra.providers` | configured provider list | same | same |

Tests `TestAdapterCapabilities::test_capabilities_truthful_for_generation_only`
and `…_for_embedding_only` lock the truth of the first two columns.
The third column (degraded) is locked by
`TestAdapterHealth::test_health_degraded_when_neither_supported`, which
proves a loaded-but-unusable model surfaces as **HEALTH_DEGRADED**
rather than a fake-healthy response.

## Error Mapping Evidence

| Condition | MAI typed error | Test |
|---|---|---|
| onnxruntime not installed | `BackendUnavailableError` | `TestAdapterLifecycle::test_initialize_backend_unavailable`, `TestClientIntegrationMocked::test_load_backend_unavailable_when_module_missing` |
| model path missing | `ModelNotFoundError` | `TestAdapterLifecycle::test_initialize_model_not_found`, `TestClientIntegrationMocked::test_load_model_not_found` |
| empty `model_path` | `ValidationError` | `TestAdapterLifecycle::test_initialize_validation_error_when_path_missing`, `TestClientIntegrationMocked::test_load_validation_error_no_path` |
| OOM during load or generate | `OutOfMemoryError` | `TestAdapterLifecycle::test_initialize_oom`, `TestAdapterGenerate::test_generate_oom_maps_to_oom`, `TestAdapterEmbed::test_embed_oom_maps` |
| generation timeout | `AdapterTimeoutError` | `TestAdapterGenerate::test_generate_timeout_maps_to_adapter_timeout` |
| native runtime exception during `run()` | `BackendCrashedError` | `TestAdapterGenerate::test_generate_backend_crash_maps_to_typed_error`, `TestClientIntegrationMocked::test_session_native_error_maps_to_backend_crashed` |
| non-numeric session output | `ValidationError` (via `OnnxRuntimeClientError("ValidationError")`) | `TestClientIntegrationMocked::test_embed_malformed_session_output_maps_to_validation` |
| backend can't generate (encoder model) | `UnsupportedOperationError` | `TestAdapterGenerate::test_generate_unsupported_when_no_generation`, `TestAdapterBatch::test_batch_unsupported_when_no_generation`, `TestClientIntegrationMocked::test_generate_stream_unsupported_without_genai` |
| backend can't embed (generation-only) | `UnsupportedOperationError` | `TestAdapterEmbed::test_embed_unsupported`, `TestClientIntegrationMocked::test_embed_unsupported_when_generation_only` |
| unknown client-error kind | `AdapterError` with original code (no raw exception leak) | `TestAdapterGenerate::test_generate_streaming_unknown_kind_maps_to_adapter_error` |

The minimum-five from harness-lock §Unit Test Minimums (unavailable
backend, timeout, model missing, unsupported embedding, one
backend-specific error) are all covered above.

## Known Limitations

1. **`onnxruntime-genai` API surface is moving.** The client uses the
   `GeneratorParams` / `Generator` / `Tokenizer.create_stream()`
   primitives from the 0.4.x / 0.5.x line. Older `onnxruntime-genai`
   builds used a different surface; the adapter would fail
   `initialize()` with `BackendUnavailableError` rather than silently
   misbehaving — that is the intended behavior, not a regression.

2. **Provider negotiation.** ONNX Runtime silently falls back through
   the provider list when a requested provider is unavailable
   (e.g. `CUDAExecutionProvider` on a CPU-only host). The adapter does
   not currently expose the actually-selected provider in
   `capabilities().extra` — only the requested list. A follow-up
   convergence session may add the post-load selected provider.

3. **No native batch path.** `generate_batch` runs prompts sequentially
   inside the adapter thread; this is honest about the lack of a
   batched `Generator` API in onnxruntime-genai. `supports_batching`
   is therefore `False`.

4. **No vision / tool-calling.** The adapter does not currently parse
   structured output schemas or function-call requests, even though
   some ONNX models support them upstream. Capability flags report
   `False` for both — capability truth, not aspirational support.

5. **In-process embedding shape assumption.** `embed()` feeds texts to
   the session under a single fixed input name `input_text`. Encoder
   models that use a different input name (e.g. `input_ids` with a
   tokenizer step in front) will surface as `BackendCrashedError`. A
   convergence session may add input-name discovery; this is documented
   here per shared-contract §Ownership Rules ("If a shared contract
   gap is discovered, document it in the adapter evidence note and
   leave the shared change for the convergence pass").

6. **No live-backend evidence yet.** See §Live Backend Result.

## Completion Verdict

**COMPLETE** against the J-24 prompt and the shared-contract /
harness-lock requirements:

- [x] `mai/adapters/onnxruntime/` package landed with adapter, client,
      config, and tests.
- [x] Scope defined honestly: in-process load, CPU default with
      DirectML/CUDA opt-in, generation only when genai supports it.
- [x] Encoder-only models raise `UnsupportedOperationError`, not fake
      success.
- [x] Unit tests with mocked `onnxruntime` and `onnxruntime_genai`
      modules — runs on a host with neither package installed.
- [x] Opt-in live tests gated by `ONNXRUNTIME_MODEL_PATH`; skip cleanly
      otherwise.
- [x] Adapter registered via `@mai_adapter(name="onnxruntime")` and
      proven by `TestRegistry::test_adapter_is_in_registry` and
      `…_runner_load_adapter`. `adapters/runner.py` unchanged.
- [x] `docs/ADAPTER-COMPLETION-MATRIX.md` deliberately **not** edited
      from this session per shared-contract §Ownership Rules — the
      convergence session rolls J-24 evidence into the matrix.
- [x] `ruff` clean, `mypy --strict` clean, `pytest` 45/45 pass.

Outside-tester live-backend exercise is the next gate.
