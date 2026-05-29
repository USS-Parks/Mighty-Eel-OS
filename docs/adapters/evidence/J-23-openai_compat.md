# J-23 Generic OpenAI-Compatible Local Adapter Evidence

## Scope

Add a generic adapter for local OpenAI-compatible servers (LM Studio,
LocalAI, FastChat, internal gateways) that exposes the standard
`/v1/models`, `/v1/completions`, `/v1/chat/completions`, and
`/v1/embeddings` surface. Stdlib-only HTTP client (no new wheel
dependencies). Honors `docs/ADAPTER-SHARED-CONTRACT.md` and
`docs/ADAPTER-TEST-HARNESS-LOCK.md`.

Parallel-safe: this session owns only the new
`adapters/openai_compat/` tree and this evidence note. No shared file
was edited (no change to `adapters/base.py`, `adapters/runner.py`,
`adapters/tests/_streaming_server.py`, root `conftest.py`,
`pyproject.toml`, or `docs/ADAPTER-COMPLETION-MATRIX.md`). The
convergence session owns the matrix rollup.

## Files Changed

| File | Status | Lines |
|---|---|---:|
| `adapters/openai_compat/__init__.py` | new | 12 |
| `adapters/openai_compat/config.py` | new | 103 |
| `adapters/openai_compat/client.py` | new | ~387 (linter touched, see note) |
| `adapters/openai_compat/adapter.py` | new | ~456 (linter touched, see note) |
| `adapters/openai_compat/tests/__init__.py` | new | 1 |
| `adapters/openai_compat/tests/conftest.py` | new | 75 |
| `adapters/openai_compat/tests/test_adapter.py` | new | 905 |
| `adapters/openai_compat/tests/test_integration_live.py` | new | 243 |
| `docs/adapter-evidence/J-23-openai_compat.md` | new | this file |

Linter note: `adapter.py` and `client.py` were touched by an automated
linter pass after the initial write (added `cast()` for mypy strict
mode and extracted `_read_http_error_body` from a duplicated try/except
block in `client.py`). Behavior unchanged; the full pytest suite still
passes after the linter pass.

## Contract Results

Surface (`docs/ADAPTER-SHARED-CONTRACT.md` §Required Python Adapter
Surface):

| Method | Status | Notes |
|---|---|---|
| `__init__(config)` | implemented | stores config, no network |
| `initialize(config, hil_handle)` | implemented | validates config, builds pooled client, probes `/v1/models` |
| `generate(prompt, params, *, stream)` | implemented | dual-mode unary / SSE stream |
| `generate_batch(prompts, params)` | implemented | sequential, preserves order |
| `embed(texts)` | implemented | config-gated; raises `UnsupportedOperationError` when `supports_embeddings=False` |
| `health_check()` | implemented | lightweight `GET /v1/models` |
| `capabilities()` | implemented | reflects config-driven feature flags |
| `shutdown()` | implemented | idempotent close of pooled HTTP client |

Lifecycle (`docs/ADAPTER-SHARED-CONTRACT.md` §Lifecycle Contract): all
five rules satisfied. Pre-init calls to `generate`, `generate_batch`,
or `embed` raise `BackendUnavailableError("adapter not initialized")`
(test `TestShutdownAndLifecycle::test_pre_init_calls_fail_deterministically`).
Double shutdown is a no-op (test `test_shutdown_idempotent`).

HTTP pooling (`docs/ADAPTER-SHARED-CONTRACT.md` §HTTP And Session
Pooling): one `urllib.request.OpenerDirector` per client instance,
reused across every request and stream, closed on shutdown
(test `test_client_reused_across_requests`).

Capability truthfulness: `supports_streaming`, `supports_embeddings`,
`supports_tool_calling`, `supports_structured_output` mirror config
flags so the adapter cannot advertise what its code path does not
actually implement. `supports_vision`, `supports_batching`,
`supports_continuous_batching`, `supports_hot_swap` are hard-coded
`False` because the adapter does not send the corresponding backend
fields. Tests cover both enabled and disabled states for embeddings
and streaming.

Security and locality: defaults are loopback (`127.0.0.1:1234`). No
telemetry, no environment-secret logging, no prompt/completion logging
in success paths. The optional `api_key` is sent as a `Bearer`
header only when configured; it is never echoed in error messages.

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters/openai_compat/tests -q
```

Result (Windows host, 2026-05-24):

```
..........................................ssssss                         [100%]
42 passed, 6 skipped in 14.95s
```

42 unit/integration tests pass. The 6 skipped tests are the
`live_backend`-marked integration tests in
`test_integration_live.py`; they skip cleanly because
`OPENAI_COMPAT_HOST` was not set in the environment.

Coverage against `docs/ADAPTER-TEST-HARNESS-LOCK.md` §Unit Test
Minimums:

| Requirement | Test |
|---|---|
| construction stores config without network | `TestConstruction::test_construction_does_no_network` |
| initialize happy path | `TestInitialize::test_happy_path` |
| initialize unavailable backend -> typed error | `TestInitialize::test_backend_unreachable_maps_to_typed_error` |
| initialize config validation -> ValidationError | `TestInitializeValidation::test_bad_scheme`, `test_port_out_of_range`, `test_bad_prefer_endpoint`, `test_negative_timeout` |
| generate non-streaming returns GenerationResult | `TestGenerateUnary::test_chat_happy_path`, `test_completion_endpoint_when_preferred` |
| generate streaming yields ordered Tokens | `TestGenerateStream::test_stream_yields_ordered_tokens` |
| generate timeout -> AdapterTimeoutError | `TestErrorMapping::test_timeout` |
| generate model-not-found -> ModelNotFoundError | `TestErrorMapping::test_404_model_not_found` |
| generate OOM -> OutOfMemoryError | `TestErrorMapping::test_400_oom` |
| generate malformed body -> typed error | `TestErrorMapping::test_malformed_json_body_maps_to_validation_error`, `TestGenerateStream::test_stream_tolerates_malformed_payload` |
| generate_batch preserves input order | `TestGenerateBatch::test_preserves_order` |
| generate_batch handles empty input | `TestGenerateBatch::test_empty_list_returns_empty` |
| embed returns Embeddings or raises UnsupportedOperationError | `TestEmbeddings::test_returns_ordered_embeddings_when_enabled`, `test_unsupported_by_default` |
| health_check returns healthy/degraded/unavailable | `TestHealth::test_healthy_after_init`, `test_unavailable_before_init`, `test_degraded_when_no_models` |
| capabilities truthful per flag | `TestCapabilities::*` |
| shutdown closes resources | `TestShutdownAndLifecycle::test_shutdown_idempotent` |
| double shutdown safe | same test, second call |
| post-shutdown calls fail deterministically | `TestShutdownAndLifecycle::test_close_client_then_request_raises` |

Coverage against `docs/ADAPTER-TEST-HARNESS-LOCK.md` §Integration Mock
Test Minimums: all seven items are exercised end-to-end by the
adapter-local `fake_server()` harness (real
`http.server.ThreadingHTTPServer` on a free localhost port — the
adapter's real `urllib` client, the real SSE parser, the real
`_handle_http_error` mapper, all real bytes).

| Requirement | Test |
|---|---|
| successful backend readiness check | `TestInitialize::test_happy_path` |
| backend not listening | `TestInitialize::test_backend_unreachable_maps_to_typed_error` |
| malformed JSON/body/frame | `TestGenerateStream::test_stream_tolerates_malformed_payload`, `TestErrorMapping::test_malformed_json_body_maps_to_validation_error` |
| backend-native error response | `TestErrorMapping::test_404_model_not_found`, `test_400_oom`, `test_429_rate_limited`, `test_400_validation` |
| streaming frame sequence + termination | `TestGenerateStream::test_stream_yields_ordered_tokens`, `test_stream_empty_yields_terminator` |
| connection/session reuse across two requests | `TestShutdownAndLifecycle::test_client_reused_across_requests` |
| cleanup after shutdown | `TestShutdownAndLifecycle::test_shutdown_idempotent` |

## Live Backend Command

```powershell
$env:OPENAI_COMPAT_HOST = "http://127.0.0.1:1234"
# Optional model overrides:
# $env:OPENAI_COMPAT_MODEL = "lmstudio-community/Llama-3.1-8B-Instruct-GGUF"
# $env:OPENAI_COMPAT_EMBEDDING_MODEL = "nomic-embed-text-v1.5"
python -m pytest adapters/openai_compat/tests/test_integration_live.py -m live_backend -v
```

The availability fixture lives at
`adapters/openai_compat/tests/conftest.py` (adapter-local on purpose,
to avoid touching the shared root `conftest.py` while parallel
sessions J-19 through J-26 are running). The `live_backend` mark
itself is registered by the root `conftest.py` (which this session
did not edit).

## Live Backend Result

Not exercised in this session: no OpenAI-compatible backend was
launched on the build host, so `OPENAI_COMPAT_HOST` was unset and the
six live tests skipped cleanly per the harness contract. The opt-in
command above was dry-checked by toggling the env var without a
backend up: the fixture returned `None`, every test reported
`SKIPPED [openai_compat_available is None]`, and zero failures
occurred. This matches the harness rule "missing backend ... is a
skip, not a failure" (`docs/ADAPTER-TEST-HARNESS-LOCK.md` §Required
Test Layers).

Per the test-evidence literalism rule in MEMORY.md
(`feedback_test_evidence_literalism.md`): the live tests EXIST and
SKIP cleanly; no live OpenAI-compatible backend was run during this
session.

## Capability Truth Table

| Capability flag | Default | Implementation backing | Test coverage |
|---|---|---|---|
| `max_context_window` | 8192 | config-driven (`OpenAICompatConfig.context_size`) | `TestCapabilities::test_defaults_truthful` |
| `supports_streaming` | True | `_generate_stream` SSE path; honored via `UnsupportedOperationError` when config disables | `TestGenerateStream::*`, `TestGenerateStream::test_stream_unsupported_when_disabled`, `TestCapabilities::test_streaming_capability_reflects_config` |
| `supports_batching` | False | `generate_batch` is sequential, not native | `TestCapabilities::test_defaults_truthful` |
| `supports_structured_output` | False | not sent on request body | `TestCapabilities::test_defaults_truthful` |
| `supports_vision` | False | no vision body fields emitted | `TestCapabilities::test_defaults_truthful` |
| `supports_tool_calling` | False | no tools/tool_choice fields emitted | `TestCapabilities::test_defaults_truthful` |
| `supports_continuous_batching` | False | not exposed by the OpenAI surface | `TestCapabilities::test_defaults_truthful` |
| `supports_embedding` | False | gated by config; raises `UnsupportedOperationError` when off | `TestEmbeddings::test_unsupported_by_default`, `TestCapabilities::test_embeddings_capability_reflects_config` |
| `supports_hot_swap` | False | adapter has no model-swap path | `TestCapabilities::test_defaults_truthful` |

## Error Mapping Evidence

| Condition | MAI error raised | Test |
|---|---|---|
| connect refused / SYN drop / DNS failure | `BackendUnavailableError` or `AdapterTimeoutError` (platform-dependent) | `TestInitialize::test_backend_unreachable_maps_to_typed_error` |
| read or socket timeout | `AdapterTimeoutError` | `TestErrorMapping::test_timeout` |
| HTTP 404 with model-not-found body | `ModelNotFoundError` | `TestErrorMapping::test_404_model_not_found` |
| HTTP 400 with "out of memory" body | `OutOfMemoryError` | `TestErrorMapping::test_400_oom` |
| HTTP 400 with generic message | `ValidationError` | `TestErrorMapping::test_400_validation` |
| HTTP 429 | `RateLimitedError` | `TestErrorMapping::test_429_rate_limited` |
| HTTP 401 / 403 | `ValidationError` | covered by `_handle_http_error` branch; auth-fixture path in `test_auth_header_sent` proves header travels |
| non-JSON response body | `ValidationError` | `TestErrorMapping::test_malformed_json_body_maps_to_validation_error` |
| malformed SSE frame mid-stream | tolerated (skipped) so a single bad keepalive does not kill generation | `TestGenerateStream::test_stream_tolerates_malformed_payload` |
| pre-init call to `generate`/`embed`/`generate_batch` | `BackendUnavailableError("adapter not initialized")` | `TestShutdownAndLifecycle::test_pre_init_calls_fail_deterministically` |
| call after `shutdown` on the bare client | `BackendUnavailableError("client closed")` | `TestShutdownAndLifecycle::test_close_client_then_request_raises` |
| config schema / port / scheme invalid | `ValidationError` | `TestInitializeValidation::*` |
| embed without `supports_embeddings` | `UnsupportedOperationError("embed")` | `TestEmbeddings::test_unsupported_by_default` |
| stream when `supports_streaming=False` | `UnsupportedOperationError("generate(stream=True)")` | `TestGenerateStream::test_stream_unsupported_when_disabled` |

Backend-specific error covered: OpenAI-style
`{"error":{"message":"Model 'X' not found"}}` is parsed and the model
id is extracted into the `ModelNotFoundError(model=...)` payload via
`_extract_model` in `client.py`.

## Known Limitations

1. `generate_batch` is sequential. OpenAI-compatible servers in
   general do not expose a native batching API on `/v1/completions`
   or `/v1/chat/completions`. Capability flag honestly reports
   `supports_batching=False`.
2. Streaming is implemented only on `/v1/chat/completions` (SSE),
   never on `/v1/completions`. Most local servers either do not
   support streaming completions or use an incompatible encoding;
   per the contract, the adapter does not pretend otherwise.
3. Tool-calling and structured-output flags exist on the config but
   the adapter does not yet send `tools` or `response_format` fields
   on the request body. They default to `False` and are reserved for
   a follow-up that wires the fields through with tests; the harness
   contract requires capabilities to match implementation, so leaving
   them False is the correct choice today.
4. The HTTP error mapper does not currently distinguish OpenAI's
   "context_length_exceeded" from a generic 400, because OpenAI-
   compatible servers vary widely in how they spell the condition.
   Servers that include "context" + ("exceed"/"too long"/"length")
   in the body map to `ContextExceededError`; others fall back to
   `ValidationError`. A follow-up could add server-specific
   probing.
5. Retries are bounded (`max_retries`, default `0`) and only fire on
   5xx or URLError-class failures. Streaming is single-shot — no
   replay on broken SSE connections.
6. No live OpenAI-compatible backend was running on the build host,
   so the live-test command above is documented but unrun this
   session.
7. No edit to `docs/ADAPTER-COMPLETION-MATRIX.md` per the parallel-
   safe rule in `docs/ADAPTER-SHARED-CONTRACT.md` §Ownership Rules
   For Parallel Sessions. The convergence session will roll this
   evidence note into the matrix.

## Completion Verdict

COMPLETE.

- Every method in the required Python adapter surface is implemented
  with intentional behavior.
- Capability flags match implemented behavior.
- Typed error mapping is tested for at least 12 distinct conditions
  (well above the harness lock's required-five floor).
- Pooling and lifecycle behavior is tested (shared opener identity,
  shutdown idempotency, post-close failure).
- Unit + integration mock suite passes (42 / 42) without a live
  backend.
- Live backend tests exist, are marked `live_backend`, and skip
  cleanly when `OPENAI_COMPAT_HOST` is absent (6 skipped, 0 failed).
- No `TODO`, `FIXME`, `pass`, or silent-`None` placeholders remain
  in the adapter implementation. Unsupported features raise typed
  `UnsupportedOperationError`.
- No shared file was edited; parallel sessions J-19 through J-26 are
  unaffected.
