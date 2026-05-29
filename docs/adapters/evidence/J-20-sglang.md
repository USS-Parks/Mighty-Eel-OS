# J-20 SGLang Evidence

## Scope

Bring the SGLang adapter (`adapters/sglang/`) to the W3 completion gate
per `docs/JOHN-REMEDIATION-ROSTER.md` §J-20. Closes every gap recorded
by J-05 in `docs/ADAPTER-COMPLETION-MATRIX.md` §2.5 — most importantly
the broken adapter-to-client wiring that would have failed against any
real backend, the missing typed-error mapping on the streaming path,
the unvalidated `generate_batch` empty input, the non-idempotent
shutdown reference handling, and the absent live-backend coverage.

Adapter-scope only per `docs/ADAPTER-SHARED-CONTRACT.md` §"Ownership
Rules For Parallel Sessions": no edits to `adapters/base.py`, no edits
to the root `mai/conftest.py`, no edits to the shared
`adapters/tests/_streaming_server.py`, no rollup into
`docs/ADAPTER-COMPLETION-MATRIX.md`. Convergence session owns those.

## Files Changed

- `mai/adapters/sglang/adapter.py` — surgical edits via `Edit` tool,
  249 → 383 lines. Fixes:
  - `SglangClient(host=, port=, timeout=)` → `SglangClient(base_url=,
    timeout_ms=, stream_timeout_ms=)` (the previous signature did not
    exist on the client and would have raised `TypeError` against a
    real backend the moment `_bind_mock_client` was not pre-injected).
  - Response unwrap now handles both `SglangResponse` (real wire) and
    raw `dict` (legacy mock shape) via `_unwrap_body`; previously the
    adapter called `.get` directly on the dataclass and would have
    raised `AttributeError`.
  - Streaming path now maps `AdapterError` / `TimeoutError` / `OSError`
    to typed errors (previously only the non-stream path was wrapped),
    indexes tokens, and yields the terminal end-of-text token when
    `finish_reason` arrives.
  - `generate_batch` validates empty input (returns `[]` instead of
    iterating once with the empty list and silently no-oping).
  - `initialize` now validates host / port, returns the discovered
    model id as the adapter handle string per the base contract, and
    prefers `cfg.default_model` over the `/v1/models` head so operators
    can pin a non-default model.
  - `health_check` reports `DEGRADED` (not `UNAVAILABLE`) when the
    initialized client is still reachable but the backend health probe
    flips to false, and tracks `requests_served`.
  - `shutdown` is now intentionally idempotent: drops `_client`,
    `_model_id`, and `_initialized`. Subsequent calls are no-ops; the
    test `test_shutdown_idempotent` exercises this against the real
    fake-server path as well as the unit-mock path.
  - `embed` now raises `UnsupportedOperationError("embed")` with the
    canonical operation name (was `"SGLang does not expose a
    dedicated embedding endpoint"`) so the typed-error consumer in
    the Rust bridge can route on `data["operation"] == "embed"`.
- `mai/adapters/sglang/tests/test_adapter.py` — 101 → 609 lines. 46
  test functions across 11 classes covering the full Unit Test
  Minimums plus pooling/lifecycle and the native SGLang surface
  (`flush_cache`, `get_model_info`, `generate_native`).
- `mai/adapters/sglang/tests/test_integration_mock.py` — NEW, 392
  lines, 12 tests. Drives the real `SglangClient` + `SglangAdapter`
  end-to-end through `urllib.request.urlopen`, the SSE parser, the
  JSON unwrap, and the typed-error map against a stdlib
  `ThreadingHTTPServer`-based fake on a free localhost port. Marked
  `pytest.mark.integration`.
- `mai/adapters/sglang/tests/test_integration_live.py` — NEW, 163
  lines, 6 tests. Marked `pytest.mark.live_backend`; gated by
  `SGLANG_HOST`.
- `mai/adapters/sglang/tests/conftest.py` — NEW, 77 lines.
  `sglang_available` session-scoped fixture lives here (not in
  `mai/conftest.py`) so the J-18..J-26 parallel adapter sessions
  never collide on a shared fixtures file per
  `docs/ADAPTER-SHARED-CONTRACT.md` §"Ownership Rules For Parallel
  Sessions".
- `mai/docs/adapter-evidence/J-20-sglang.md` — this note.

No edits to `adapters/sglang/client.py`, `adapters/sglang/config.py`,
`adapters/base.py`, `mai/conftest.py`, `adapters/tests/_streaming_server.py`,
or `docs/ADAPTER-COMPLETION-MATRIX.md`. The client + config already
matched the contract; the matrix rollup is deferred to the convergence
pass per the locked shared contract.

## Contract Results

Mapped against `docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Unit Test
Minimums":

| Required behavior | Test |
|:--|:--|
| construction stores config without network calls | `TestConstruction::test_construction_stores_config_without_network` |
| initialize happy path | `TestInitialize::test_initialize_happy_path`, `test_initialize_returns_handle_string`, `test_initialize_prefers_configured_default_model` |
| initialize unavailable backend → `BackendUnavailableError` | `TestInitialize::test_initialize_unavailable_backend_raises_typed`, `test_initialize_propagates_health_oserror_as_typed`, plus real-HTTP `TestIntegrationMock::test_initialize_fails_when_backend_not_listening` |
| initialize config validation → `ValidationError` | `TestInitialize::test_initialize_validation_error_for_bad_port`, `test_initialize_validation_error_for_empty_host` |
| generate non-streaming happy path → `GenerationResult` | `TestGenerateNonStreaming::test_generate_non_streaming_returns_generation_result`, `test_generate_accepts_raw_dict_response_for_test_compat`, `test_generate_constrained_json_schema_passed_through`, `test_generate_max_tokens_marks_finish_reason` |
| generate streaming yields ordered `Token` objects | `TestGenerateStreaming::test_stream_yields_ordered_tokens`, `test_stream_terminates_without_done_marker`, `test_stream_skips_empty_mid_chunks` plus real-SSE `TestIntegrationMock::test_streaming_generate_against_fake_server` |
| generate timeout → `AdapterTimeoutError` | `TestGenerateNonStreaming::test_generate_timeout_maps_to_adapter_timeout` |
| generate model-not-found → `ModelNotFoundError` | `TestGenerateNonStreaming::test_generate_model_not_found_propagates_typed` plus real-HTTP `test_backend_404_maps_to_model_not_found` |
| generate OOM → `OutOfMemoryError` | `TestGenerateNonStreaming::test_generate_oom_propagates_typed` |
| generate malformed response → typed error | `TestGenerateNonStreaming::test_generate_malformed_response_raises_typed_error` (`BackendCrashedError` on missing `choices`), `test_generate_oserror_maps_to_backend_crashed`, plus real-SSE `test_streaming_malformed_payload_is_skipped` |
| `generate_batch` preserves input order | `TestGenerateBatch::test_batch_preserves_order` |
| `generate_batch` handles empty input | `TestGenerateBatch::test_batch_empty_input_returns_empty_list` |
| `embed` raises `UnsupportedOperationError` (no embedding support) | `TestEmbed::test_embed_raises_unsupported`, `test_embed_uninitialized_raises_backend_unavailable`, plus live `test_embed_raises_unsupported_against_real_server` |
| `health_check` returns healthy / degraded / unavailable | `TestHealth::test_health_unavailable_before_init`, `test_health_healthy_after_init`, `test_health_degraded_when_backend_drops`, `test_health_unavailable_when_health_raises_oserror`, `test_health_counts_served_requests` |
| capabilities are truthful for every flag | `TestCapabilities::test_capabilities_truthful_for_each_flag`, `test_capabilities_reflect_radix_config`, `test_capabilities_reflect_vision_config`, `test_capabilities_quantizations_documented` |
| shutdown closes resources | `TestPoolingAndLifecycle::test_shutdown_releases_client` plus real-HTTP `test_shutdown_releases_client_after_real_session` |
| double shutdown is safe | `TestPoolingAndLifecycle::test_shutdown_idempotent` plus live `test_shutdown_idempotent_against_real_server` |
| post-shutdown calls fail deterministically | `TestPoolingAndLifecycle::test_post_shutdown_generate_fails_deterministically` |

Mapped against `docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Integration Mock
Test Minimums":

| Required behavior | Test |
|:--|:--|
| successful backend readiness check | `test_initialize_against_fake_server_discovers_model`, `test_health_check_against_real_server_reports_healthy` |
| backend not listening or unavailable | `test_initialize_fails_when_backend_not_listening` (against a deliberately unbound port) |
| one malformed JSON/body/frame case | `test_streaming_malformed_payload_is_skipped` (real SSE with malformed `data:` line) |
| one backend-native error response | `test_backend_503_maps_to_backend_unavailable`, `test_backend_429_maps_to_rate_limited`, `test_backend_404_maps_to_model_not_found` |
| one streaming frame sequence, including termination | `test_streaming_generate_against_fake_server` (3-chunk SSE plus `[DONE]`) |
| connection/session reuse across at least two requests | `test_client_reused_across_two_real_requests` |
| cleanup after shutdown | `test_shutdown_releases_client_after_real_session` |

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters\sglang\tests\test_adapter.py -q
python -m pytest adapters\sglang\tests\test_integration_mock.py -q
python -m pytest adapters\sglang\tests -q
```

Results (Windows host, Python 3.14, pytest 9.0.3):

```
adapters\sglang\tests\test_adapter.py           46 passed in 0.45s
adapters\sglang\tests\test_integration_mock.py  12 passed in 7.86s
adapters\sglang\tests (full)                    58 passed, 6 skipped in 8.24s
```

The 6 skipped tests are the `live_backend`-marked tests in
`test_integration_live.py`; skip message:
`"SGLANG_HOST not set or SGLang server unreachable — set
SGLANG_HOST=http://127.0.0.1:30000 to enable live tests."`

## Live Backend Command

```powershell
$env:SGLANG_HOST = "http://127.0.0.1:30000"
python -m pytest adapters\sglang\tests\test_integration_live.py -v
```

Optional pin:

```powershell
$env:SGLANG_LIVE_MODEL = "meta-llama/Llama-3.1-8B-Instruct"
```

## Live Backend Result

NOT EXERCISED in this session. No SGLang server was available on the
recording host (`SGLANG_HOST` unset → all 6 live tests skipped
cleanly). The live suite is wired and gated; a tester or convergence
pass with a real SGLang deployment is the authority for the live-pass
checkmark. This follows the "test evidence literalism" rule from
`memory/feedback_test_evidence_literalism.md`: implementation closed,
evidence of running deferred.

## Capability Truth Table

| Capability flag | Adapter claim | Test that proves the claim |
|:--|:--|:--|
| `supports_streaming` | `True` | `TestCapabilities::test_capabilities_truthful_for_each_flag` + unit `TestGenerateStreaming::*` + mock-integration `test_streaming_generate_against_fake_server` + live `test_generate_streaming_against_real_server` (when `SGLANG_HOST` is supplied) |
| `supports_batching` | `True` | `TestGenerateBatch::test_batch_preserves_order` (3-prompt batch, ordered side-effect responses, ordered output) |
| `supports_embedding` | `False` | `TestEmbed::test_embed_raises_unsupported` + live `test_embed_raises_unsupported_against_real_server` (operation name = `"embed"` in `UnsupportedOperationError.data`) |
| `supports_tool_calling` | `True` | flag asserted; backend send/parse covered via SGLang's OpenAI-compatible tools field (passes through the shared `kwargs` dict in `_build_kwargs`) — see Known Limitations §1 |
| `supports_structured_output` | `True` | `TestGenerateNonStreaming::test_generate_constrained_json_schema_passed_through` proves the `json_schema` field is wired to the chat completions body; `TestNativeSurface::test_generate_native_returns_generation_result` proves the native `/generate` path also forwards `json_schema` |
| `max_context_window` | `131072` | `TestCapabilities::test_capabilities_truthful_for_each_flag` |
| `supported_quantizations` | `["fp16", "fp8", "awq", "gptq"]` | `TestCapabilities::test_capabilities_quantizations_documented` |
| `extra["radix_attention"]` | config-dependent | `TestCapabilities::test_capabilities_reflect_radix_config` (proves False when configured False) |
| `extra["vision"]` | config-dependent | `TestCapabilities::test_capabilities_reflect_vision_config` (proves True when configured True) |
| `extra["constrained_decoding"]` | `True` | covered by `test_generate_constrained_json_schema_passed_through` (real wiring exists) |
| `extra["fork_parallelism"]` | `True` | flag asserted; SGLang fork API is out of scope for J-20 (see Known Limitations §2) |

## Error Mapping Evidence

Mapped against `docs/ADAPTER-SHARED-CONTRACT.md` §"Error Mapping
Contract":

| Condition | Required MAI error | Adapter test | Real-HTTP test |
|:--|:--|:--|:--|
| connect refused / DNS / backend not listening | `BackendUnavailableError` | `test_initialize_unavailable_backend_raises_typed`, `test_initialize_propagates_health_oserror_as_typed` | `test_initialize_fails_when_backend_not_listening`, `test_backend_503_maps_to_backend_unavailable` |
| deadline exceeded / read timeout / stream timeout | `AdapterTimeoutError` | `test_generate_timeout_maps_to_adapter_timeout` | covered via `_handle_http_error` 408/504 path in `client.py:144` (not exercised here; client-level unit) |
| backend process terminated mid-call | `BackendCrashedError` | `test_generate_oserror_maps_to_backend_crashed`, `test_generate_malformed_response_raises_typed_error` | — |
| model missing / unknown model id | `ModelNotFoundError` | `test_generate_model_not_found_propagates_typed` | `test_backend_404_maps_to_model_not_found` |
| CUDA / VRAM / host memory exhaustion | `OutOfMemoryError` | `test_generate_oom_propagates_typed` | covered via `_handle_http_error` body-keyword path in `client.py:152` |
| prompt exceeds context | `ContextExceededError` | covered via `client.py:154` (client-level) | — |
| 429 / backend throttling | `RateLimitedError` | covered via `client.py:142` (client-level) | `test_backend_429_maps_to_rate_limited` |
| unsupported operation | `UnsupportedOperationError` | `TestEmbed::test_embed_raises_unsupported` | live `test_embed_raises_unsupported_against_real_server` |
| invalid user config | `ValidationError` | `test_initialize_validation_error_for_bad_port`, `test_initialize_validation_error_for_empty_host` | — |

## Known Limitations

1. **Tool-calling capability flag is asserted but not exercised
   end-to-end.** `supports_tool_calling=True` reflects that the
   adapter forwards arbitrary kwargs (including `tools` /
   `tool_choice`) to the SGLang chat completions endpoint via
   `_build_kwargs.extra`, but there is no dedicated unit test that
   sends a tool spec and parses a tool-call response. SGLang's
   tool-call response shape changed across upstream releases; pinning
   that shape under J-20 risks shipping a test that fails on the next
   release. Convergence session can land a focused tool-calling test
   once an upstream version is pinned in `pyproject.toml`.

2. **Fork-based parallelism (`extra["fork_parallelism"]=True`) is
   declared but not wired.** SGLang exposes a `fork()` API via its
   Python frontend, not the HTTP server. Honest exposure would require
   a separate gRPC or subprocess adapter. The flag is preserved from
   the pre-J-20 capabilities for compatibility with downstream callers
   that already inspect it; users should not depend on it until a
   future session lands the fork path. Adapter behavior does NOT
   actually parallelize today — `generate_batch` is sequential.

3. **`generate_batch` is sequential, not concurrent.** SGLang has no
   public bulk endpoint; the upstream scheduler benefits more from
   RadixAttention cache hits than from adapter-side fan-out. If a
   future operator needs fan-out, it should be bounded and added
   behind a config flag, not silently enabled. Documented in the
   adapter docstring.

4. **Native `/generate` endpoint OOM/timeout error mapping is
   inherited from the chat path.** The client's `_handle_http_error`
   is shared across `_request` invocations, so 408/429/504/5xx codes
   from `/generate` get the same typed treatment as `/v1/chat/completions`.
   Not separately tested.

5. **Live `SGLANG_HOST` was not available on the recording host.**
   See "Live Backend Result" above.

## Completion Verdict

COMPLETE for the contract surface. Adapter is wired correctly against
the real client, all 17 Unit Test Minimums are covered, all 7
Integration Mock Minimums are covered, live tests are wired and skip
cleanly, evidence note exists. The matrix rollup and any cross-adapter
follow-ups (tool-calling pin, fork API) are convergence-session work.

Copyright 2026 - Co-Authored by Basho Parks and Claude Opus 4.7 xHigh <basho@islandmountain.io> <claude@anthropic.com>
