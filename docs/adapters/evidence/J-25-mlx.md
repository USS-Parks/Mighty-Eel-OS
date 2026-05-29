# J-25 MLX Evidence

## Scope

Add the Apple Silicon local-inference MLX adapter under `mai/adapters/mlx/`.
Conforms to the locked shared contract (`docs/ADAPTER-SHARED-CONTRACT.md`)
and harness (`docs/ADAPTER-TEST-HARNESS-LOCK.md`) for the parallel
J-18..J-26 adapter-completion lane. Registration is via the existing
`@mai_adapter(name="mlx")` decorator, so the adapter is discovered by
`adapters/runner.py`'s `importlib.import_module(f"adapters.{name}.adapter")`
path without any edit to `adapters/__init__.py` (shared file under
the J-25 ownership rules).

## Files Changed

New (J-25 owned):

- `mai/adapters/mlx/__init__.py` — package exports.
- `mai/adapters/mlx/config.py` — `MLXConfig` dataclass; local model path,
  generation knobs, timeouts, batch cap, max-context-window report.
- `mai/adapters/mlx/client.py` — `MLXClient` lazy-import wrapper around
  `mlx_lm`. Imports the real package only when `load()` is called AND
  the runtime is Apple Silicon; tests inject `mlx_module=<fake>` to
  bypass both checks deterministically. Exposes `load`, `generate`,
  `stream_generate`, `close`, `loaded`, `backend_version`,
  `is_apple_silicon()`.
- `mai/adapters/mlx/adapter.py` — `MLXAdapter(AdapterBase)` with the
  full required surface; maps `MLXLoadError` to typed MAI errors;
  applies `asyncio.wait_for` budget against `timeout_ms` for non-stream
  and a wall-clock budget for streams.
- `mai/adapters/mlx/tests/__init__.py` — test package marker.
- `mai/adapters/mlx/tests/test_adapter.py` — 36 unit tests covering
  every required behavior in `ADAPTER-TEST-HARNESS-LOCK.md` §Unit Test
  Minimums.
- `mai/adapters/mlx/tests/test_integration_live.py` — 5 opt-in
  `live_backend` tests gated by `MLX_MODEL_PATH` + Apple Silicon +
  `mlx_lm` import.

Not touched (deliberate, per parallel-merge rules):

- `mai/adapters/base.py` — shared.
- `mai/adapters/runner.py` — shared; existing decorator-based discovery
  already imports `adapters.mlx.adapter` on demand.
- `mai/adapters/__init__.py` — shared; module docstring only.
- `mai/docs/ADAPTER-COMPLETION-MATRIX.md` — convergence-session owned.
- `mai/conftest.py` — root fixtures; no MLX-specific shared fixture
  is needed because MLX gate logic is self-contained inside the live
  test module's `_live_gate()` function.

## Contract Results

| Contract clause | Status | Note |
|---|---|---|
| Required surface (init/generate/generate_batch/embed/health/capabilities/shutdown) | PASS | Every method implemented intentionally; `embed` raises `UnsupportedOperationError("embed")`. |
| `__init__` is config-only, no network/sockets/model load | PASS | Stores config + builds typed `MLXConfig`; `load()` only fires from `initialize()` via `asyncio.to_thread`. |
| `initialize` validates config, prepares backend, returns handle | PASS | Returns `f"mlx-{start_ms}"`. Raises `ValidationError` for empty `model_path`. |
| Calls before init raise typed errors | PASS | `_ensure_initialized` raises `BackendUnavailableError`. |
| `shutdown` is idempotent | PASS | `MLXClient.close()` clears handles; adapter sets `_client=None`; second call is a no-op. |
| Client/session reuse for adapter lifetime | PASS | One `MLXClient` instance per adapter; `load()` is idempotent. |
| Streaming + non-streaming timeouts separate | PASS | `timeout_ms` for `generate()`; `stream_timeout_ms` enforced via monotonic deadline inside `_generate_stream`. |
| Typed-error mapping for unavailable/timeout/missing-model/OOM/unsupported | PASS | `BackendUnavailableError` (mlx-lm absent or wrong platform), `AdapterTimeoutError`, `ModelNotFoundError`, `UnsupportedOperationError`. No OOM mapping (mlx-lm does not surface a stable OOM signal; documented under Known Limitations). |
| Capabilities truthful | PASS | `supports_embedding=False`, `supports_tool_calling=False`, `supports_vision=False`, `supports_structured_output=False`, `supports_continuous_batching=False`, `supports_hot_swap=False`. `supports_streaming=True` and `supports_batching=True` are exercised by tests. |
| Generation contract (`text`, `tokens_generated`, `FinishReason`) | PASS | Non-stream returns `GenerationResult`; stream yields ordered `Token`s plus a terminal `is_end_of_text=True` sentinel. |
| Batch preserves order + handles empty list | PASS | `test_generate_batch_preserves_order` + `test_generate_batch_empty`. |
| Embedding contract | PASS | Raises `UnsupportedOperationError("embed")`; no fake vectors. |
| Health behavior (healthy/degraded/unavailable) | PASS | Three explicit tests. |
| Security: loopback-or-local default | PASS | Backend is a local filesystem path; no network surface. |
| Completion definition (no TODO/stub language) | PASS | `grep -nE 'TODO|FIXME|XXX|stub' adapters/mlx` returns no hits. |

## Unit And Mock Integration Commands

Run from `mai/`:

```powershell
python -m pytest adapters\mlx\tests\test_adapter.py -q
```

Result on this workstation (Windows 11, native Python):

```
36 passed in 0.52s
```

Full adapter-tree sanity (excluding MLX):

```powershell
python -m pytest adapters -q --ignore=adapters\mlx
```

Result: 433 passed, 37 skipped, 1 failed. The single failure
(`adapters/openai_compat/tests/test_adapter.py::TestInitialize::test_backend_unavailable_when_port_closed`)
is a J-23-owned timing test that depends on OS port-closed behavior; it
is unrelated to anything J-25 touched and is left for the J-23 session.

## Live Backend Command

```powershell
$env:MLX_MODEL_PATH = "/path/to/mlx-community/Mistral-7B-Instruct-v0.3-4bit"
python -m pytest adapters\mlx\tests\test_integration_live.py -m live_backend -q
```

## Live Backend Result

Not exercised in this session — workstation is Windows 11 on x86_64
without `mlx_lm` installable. The live suite was collected and skipped
cleanly:

```
python -m pytest adapters/mlx/tests/test_integration_live.py -q
sssss
5 skipped in 0.07s
```

Skip reason emitted by `_live_gate()`:
`MLX live test gate not satisfied (need MLX_MODEL_PATH, Apple Silicon, and mlx_lm installed)`.

Apple Silicon evidence is therefore deferred to the next Mac-equipped
tester run (J-14 re-scan or a follow-up J-25 live pass). The skip-clean
behavior is the contractually required CI behavior.

## Capability Truth Table

| Flag | Reported | Exercised | Notes |
|---|---|---|---|
| `supports_streaming` | true | yes (`test_generate_streaming_order_and_terminal`, `test_generate_streaming_drops_empty_chunks`) | terminal sentinel emitted |
| `supports_batching` | true | yes (`test_generate_batch_preserves_order`) | bounded sequential — documented |
| `supports_embedding` | false | yes (raises `UnsupportedOperationError`) | mlx-lm has no stable embedding endpoint |
| `supports_structured_output` | false | n/a | not implemented |
| `supports_tool_calling` | false | n/a | not implemented |
| `supports_vision` | false | n/a | not implemented |
| `supports_continuous_batching` | false | n/a | mlx-lm does not expose continuous batching |
| `supports_hot_swap` | false | n/a | model is one-shot per adapter instance |
| `max_context_window` | 8192 (operator-set) | indirect | mlx-lm has no stable context-window query |
| `backend_version` | `mlx_lm.__version__` after load, else `"unknown"` | yes (`test_capabilities_backend_version_*`) | |
| `extra.in_process` | true | yes (`test_capabilities_truthful`) | |
| `extra.apple_silicon_only` | true | yes | |
| `extra.platform_ok` | runtime-detected | yes | reflects `is_apple_silicon()` |

## Error Mapping Evidence

| Backend condition | MAI error raised | Test |
|---|---|---|
| `model_path` empty / missing | `ValidationError` | `test_initialize_validates_model_path` |
| mlx-lm not installed / wrong platform | `BackendUnavailableError` | `test_initialize_backend_unavailable` |
| `FileNotFoundError` from `mlx_lm.load` | `ModelNotFoundError` | `test_initialize_model_not_found` |
| non-stream generation wall-clock timeout | `AdapterTimeoutError` | `test_generate_timeout_maps_to_typed_error` |
| mid-call `MLXLoadError` (client lost) | `BackendUnavailableError` | `test_generate_backend_crash_during_call` |
| call before initialize | `BackendUnavailableError` | `test_call_before_init_raises` |
| embed call | `UnsupportedOperationError` | `test_embed_raises_unsupported` |
| client load before path set | `MLXLoadError` (client) | `test_load_empty_path_raises` |

## Known Limitations

1. **No OOM mapping.** mlx-lm does not surface a stable OOM signal in a
   form the adapter can detect without parsing exception strings; any
   real OOM presents as `MLXLoadError` mid-call and is mapped to
   `BackendUnavailableError`. A future shared-contract revision could
   widen the OOM detection rule once mlx-lm exposes a typed signal.
2. **No native batching.** `generate_batch` is bounded sequential fan-out
   over `generate`. The contract permits this when documented; the
   adapter's `max_batch_size` is reported but not currently enforced as
   a hard cap because per-call concurrency is one.
3. **Context window is operator-set.** mlx-lm has no stable query for the
   loaded model's actual context window, so `MLXConfig.max_context_window`
   defaults to 8192 and is reported as-is. Operators must override when
   serving models with smaller or larger windows.
4. **Live coverage deferred.** This session ran on Windows; the five
   live tests skip cleanly and have not been exercised against a real
   mlx-lm model. The next macOS-equipped run should set
   `MLX_MODEL_PATH` and confirm.
5. **Token-count estimation.** `generate()` reports `tokens_generated`
   from the tokenizer when available, falling back to `len(text)//4`.
   This is best-effort and is documented in `client.py:_estimate_tokens`.

## Completion Verdict

PASS for non-live contract coverage. Live-backend coverage is
contractually deferred-with-skip-clean evidence; the live suite is
written, collected, and skips with a single explicit reason on every
non-Apple host. J-25 is ready for J-14 re-scan once a macOS tester
runs the live suite or the convergence session rolls the evidence
into `docs/ADAPTER-COMPLETION-MATRIX.md`.
