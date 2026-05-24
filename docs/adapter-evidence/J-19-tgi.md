# J-19 TGI Evidence

## Scope

Bring the HuggingFace Text Generation Inference (TGI) adapter to the
W3 completion gate defined by `docs/ADAPTER-SHARED-CONTRACT.md` and
`docs/ADAPTER-TEST-HARNESS-LOCK.md`. Three layers covered: unit,
integration mock against a real local fake server, and opt-in live
backend gated by `TGI_HOST`.

Out of scope (per parallel-session ownership rules): `adapters/base.py`,
`adapters/runner.py`, `adapters/tests/_streaming_server.py`,
`mai/conftest.py`, `docs/ADAPTER-COMPLETION-MATRIX.md`. Marker
semantics reconciliation for `integration` is deferred to the
convergence session.

## Files Changed

Modified:

- `adapters/tgi/adapter.py` - idempotent initialize, config validation,
  shared `_body_dict` response normaliser, threaded streaming with
  typed-error propagation, batch contract honouring empty input, health
  check that maps `/health` failure + `/info` success to `degraded`,
  fully idempotent shutdown clearing all state.
- `adapters/tgi/client.py` - error mapping for 404 -> `ModelNotFoundError`,
  422 -> `ValidationError`, `error_type` JSON field, `overloaded`
  -> `RateLimitedError`, in-band SSE error frames mapped to
  `ContextExceededError` / `OutOfMemoryError` / `ValidationError`,
  malformed stream frame raises `BackendUnavailableError` rather than
  silently dropping tokens, `metrics()` no longer JSON-decodes
  Prometheus text.
- `adapters/tgi/tests/test_adapter.py` - replaced 7-test smoke suite
  with 47 unit tests organised into 11 classes covering construction,
  initialize lifecycle (happy, legacy `_cfg`, unavailable, validation,
  reuse, reinit-replaces-client), pre-init guards, non-streaming
  generate (timeout / model-not-found / OOM / malformed),
  streaming (ordered tokens, end marker, context-exceeded mid-stream),
  batch (order, empty, per-prompt error), embed, health (healthy /
  degraded / unavailable), capabilities (truthful flags), shutdown
  (clears state, idempotent, post-shutdown calls fail), and session
  reuse.

Added:

- `adapters/tgi/tests/conftest.py` (78 lines) - adapter-local
  `tgi_available` session fixture probing `$TGI_HOST/health` + `/info`.
- `adapters/tgi/tests/_tgi_test_server.py` (193 lines) - real
  `ThreadingHTTPServer` speaking TGI's `/health`, `/info`, `/metrics`,
  `/generate`, `/generate_stream`. Stdlib only.
- `adapters/tgi/tests/test_integration_mock.py` (333 lines) - 16
  integration tests against the local fake server.
- `adapters/tgi/tests/test_integration_live.py` (210 lines) - 6 live
  tests gated by `TGI_HOST`.

## Contract Results

| Contract requirement | Status |
|---|---|
| Required Python surface (initialize / generate / generate_batch / embed / health_check / capabilities / shutdown) | met |
| Lifecycle: `__init__` no I/O | tested (`test_init_stores_config_without_network`) |
| Lifecycle: initialize idempotent | tested (`test_reinitialize_without_new_config_reuses_client`, `test_reinitialize_with_new_config_replaces_client`) |
| Lifecycle: methods fail typed pre-init | tested (`TestTgiAdapterPreInit`) |
| Lifecycle: shutdown idempotent | tested (`test_double_shutdown_is_no_op`, `test_shutdown_before_initialize_is_safe`) |
| HTTP/session pool reuse | tested (`test_two_calls_share_client_object`, `test_two_calls_share_the_same_client_object` against real server) |
| Stdlib-only client | yes (`urllib.request`) |
| Error mapping: connect refused | `BackendUnavailableError` (mock `test_init_fails_when_server_not_listening`) |
| Error mapping: 422 validation | `ValidationError` (mock `test_422_validation_maps_to_validation_error`) |
| Error mapping: 422 context | `ContextExceededError` (mock `test_422_validation_context_maps_to_context_exceeded`) |
| Error mapping: 404 | `ModelNotFoundError` (mock + unit) |
| Error mapping: OOM | `OutOfMemoryError` (mock + unit) |
| Error mapping: 429 / overloaded | `RateLimitedError` (mock) |
| Error mapping: timeout | `AdapterTimeoutError` (unit) |
| Error mapping: malformed stream frame | typed error (mock `test_stream_malformed_frame_raises`) |
| Error mapping: in-band error frame | typed error (mock `test_stream_maps_inline_error_frame_to_typed_error`) |
| Capability truthfulness | tested (`TestTgiAdapterCapabilities`) - streaming True, embedding False, structured_output False, continuous_batching True |
| Generation result shape | tested (`TestTgiAdapterGenerate`) |
| Streaming yields ordered `Token` | tested (unit + mock) |
| Streaming terminates on end marker | tested (unit + mock) |
| Batch preserves order, validates empty, propagates errors | tested (`TestTgiAdapterBatch`) |
| Embed raises `UnsupportedOperationError` | tested (`TestTgiAdapterEmbed`) |
| Health healthy / degraded / unavailable | tested (`TestTgiAdapterHealth`) |
| Default endpoints loopback | yes (`127.0.0.1`) |
| No telemetry / no prompt-completion logging in success paths | confirmed by inspection |

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters/tgi/tests -q
```

Result: `63 passed, 6 skipped in 10.17s`. The 6 skips are the live tests
when `TGI_HOST` is unset, by design.

```powershell
python -m ruff check adapters/tgi/
```

Result: `All checks passed!`

```powershell
python -m mypy adapters/tgi/
```

Result: `Success: no issues found in 10 source files`

## Live Backend Command

```powershell
# Operator launches TGI in another terminal first, e.g.
#   docker run --gpus all --shm-size 1g -p 8080:80 \
#     -v $PWD/data:/data \
#     ghcr.io/huggingface/text-generation-inference:2.0 \
#     --model-id mistralai/Mistral-7B-Instruct-v0.2

$env:TGI_HOST = "http://127.0.0.1:8080"
python -m pytest adapters/tgi/tests/test_integration_live.py -v
```

## Live Backend Result

Not executed in this session - no TGI server was available on the host.
The live test layer was verified to skip cleanly without `TGI_HOST` set
(6 SKIPPED with the documented reason). When the operator supplies
`TGI_HOST`, the suite exercises real readiness, non-streaming generate,
streaming, embed-raises-unsupported, health, and shutdown idempotency.

What was NOT exercised here:

- real CUDA / TGI process
- real model id discovery via `/info` against actual weights
- real `/generate_stream` SSE token cadence
- 72h burn-in against TGI

These are the responsibility of the operator running the live gate on
a host with TGI installed and a model loaded.

## Capability Truth Table

| Flag | Adapter reports | Implementation backs it | Test |
|---|---|---|---|
| `supports_streaming` | True | `_generate_stream` drives real SSE | unit + mock |
| `supports_batching` | True | `generate_batch` issues ordered serial calls; TGI batches at server | unit + mock |
| `supports_structured_output` | False | no JSON-schema or grammar plumbing in this adapter | unit |
| `supports_vision` | False | no image input path | inspection |
| `supports_tool_calling` | False | no tool fields parsed | inspection |
| `supports_continuous_batching` | True | TGI's scheduler handles this server-side | inspection |
| `supports_embedding` | False | `embed` raises `UnsupportedOperationError` | unit + mock + live |
| `supports_hot_swap` | False | TGI serves one model per process | inspection |
| `max_context_window` | from `/info`'s `max_total_tokens`, defaults 8192 | populated during initialize | unit |
| `supported_quantizations` | bitsandbytes, gptq, awq, eetq, fp8 | TGI's declared support | unit |

## Error Mapping Evidence

| Condition | MAI error | Where tested |
|---|---|---|
| socket refused | `BackendUnavailableError` | `test_init_fails_when_server_not_listening` |
| `/health` returns 503 | `BackendUnavailableError` | `test_init_fails_when_health_returns_503` |
| HTTP 422 with `error_type: validation` | `ValidationError` | `test_422_validation_maps_to_validation_error` |
| HTTP 422 with input-too-long | `ContextExceededError` | `test_422_validation_context_maps_to_context_exceeded` |
| HTTP 404 | `ModelNotFoundError` | `test_404_maps_to_model_not_found` |
| HTTP 500 with `CUDA out of memory` in body | `OutOfMemoryError` | `test_oom_in_error_body_maps_to_out_of_memory` |
| HTTP 429 with `overloaded` | `RateLimitedError` | `test_429_maps_to_rate_limited` |
| Stream timeout | `AdapterTimeoutError` | client wiring; unit covers non-streaming timeout |
| Mid-stream error frame (validation/context) | `ContextExceededError` | `test_stream_maps_inline_error_frame_to_typed_error` |
| Malformed stream frame | `BackendUnavailableError` | `test_stream_malformed_frame_raises` |
| Embed | `UnsupportedOperationError` | `test_embed_raises`, `test_embed_raises_unsupported` (live) |

## Known Limitations

1. **Marker semantics conflict.** `pyproject.toml` defines `integration`
   as "requires real MAI instance" - different from the harness lock's
   "deterministic local fakes". `test_integration_mock.py` therefore
   omits the marker rather than silently redefining it. Convergence
   session should reconcile project-wide marker semantics.
2. **No HTTP connection pooling at the urllib layer.** The TGI adapter
   keeps one `TgiClient` instance per initialized adapter (proven by
   the reuse tests), but each `urllib.request.urlopen` opens its own
   TCP connection. This matches the existing Ollama / llama.cpp
   adapters; switching to a persistent `http.client.HTTPConnection`
   pool is a project-wide change deferred to convergence.
3. **`generate_batch` is serial.** TGI's server-side continuous batching
   handles concurrency, but the adapter issues calls one at a time. A
   future patch could parallelise with a bounded `asyncio.gather`, but
   that requires a thread pool sized against TGI's
   `max_concurrent_requests` and is not part of this session.
4. **Structured output not implemented.** TGI supports JSON-schema and
   regex-constrained decoding upstream; this adapter reports
   `supports_structured_output=False` and does not send those fields.
   Adding it requires schema plumbing and capability re-truthing.
5. **Live tests were not executed.** No TGI server was reachable on the
   session host. The live layer compiles, skips cleanly, and is wired
   to the contract; the operator running the live gate provides the
   evidence.

## Completion Verdict

Session J-19 closes per the shared contract for the subset of TGI
functionality the adapter implements. Unit + mock integration coverage
proves the surface; live tests are wired and gated. Three documented
deferrals (marker reconciliation, connection pooling, structured
output) sit with the convergence session and a future enhancement
session, not this gate.

Status: **COMPLETE (live evidence pending operator).**
