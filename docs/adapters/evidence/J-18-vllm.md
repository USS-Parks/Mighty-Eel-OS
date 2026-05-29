# J-18 vLLM Evidence

## Scope

Bring the vLLM adapter (`adapters/vllm/`) to the W3 completion gate per
`docs/JOHN-REMEDIATION-ROSTER.md` §J-18. Closes the live-test gap and
the embed-return-type bug surfaced by J-05 in
`docs/ADAPTER-COMPLETION-MATRIX.md` §2.3 + §3 finding #1.

Adapter-scope only per `docs/ADAPTER-SHARED-CONTRACT.md` §"Ownership
Rules For Parallel Sessions": no edits to `adapters/base.py`, no shared
conftest fixtures, no `ADAPTER-COMPLETION-MATRIX.md` rollup. Convergence
session owns those rolls.

## Files Changed

- `mai/adapters/vllm/tests/test_adapter.py` — expanded from 99 lines /
  16 assertions to 665 lines / 50 tests across three test classes
  (TestVllmConfig, TestVllmAdapter, TestVllmStreaming,
  TestVllmClientErrorMapping). Closed embed-return-type assertion bug
  (was `result[0] == [0.1, 0.2, 0.3]` against an `Embedding` dataclass;
  only passed because `Embedding.__eq__` is permissive against lists).
  Also fixed the `_model_id` → `_model` artifact in three tests (the
  adapter uses `self._model`, not `self._model_id`; the artifact was
  silently no-op).
- `mai/adapters/vllm/tests/test_integration_live.py` — new file (369
  lines, 7 tests). Inline `VLLM_HOST` probe (module-scoped fixture, no
  shared-conftest dependency). All tests `pytest.mark.live_backend`.
- `mai/docs/adapter-evidence/J-18-vllm.md` — this note.

No edits to `adapters/vllm/adapter.py`, `adapters/vllm/client.py`,
`adapters/vllm/config.py`, `mai/conftest.py`, or
`docs/ADAPTER-COMPLETION-MATRIX.md`. The adapter implementation already
met the contract surface (matrix §2.3 verdict was COMPLETE pending
test work).

## Contract Results

Mapped against `docs/ADAPTER-TEST-HARNESS-LOCK.md` §"Unit Test
Minimums":

| Required behavior | Test |
|:--|:--|
| construction stores config without network calls | `TestVllmConfig::test_defaults`, `test_custom` (plus the `__init__` reading at `adapter.py:46-53` confirms only attribute initialization) |
| initialize happy path | `test_initialize`, `test_initialize_picks_partial_match_model`, `test_initialize_falls_back_to_first_model` |
| initialize unavailable backend → `BackendUnavailableError` | `test_initialize_backend_unavailable` |
| initialize config validation → `ValidationError` | NOT TESTED — see Known Limitations §1 |
| generate non-streaming happy path | `test_generate`, `test_generate_max_tokens_finish`, `test_generate_empty_choices` |
| generate streaming yields ordered Tokens | `TestVllmStreaming::test_yields_tokens_in_order` (real-HTTP, no mock) |
| generate timeout → `AdapterTimeoutError` | `test_generate_propagates_client_timeout` + `TestVllmClientErrorMapping::test_408_maps_to_timeout`, `test_504_maps_to_timeout` |
| generate model-not-found → `ModelNotFoundError` | `test_generate_propagates_model_not_found` + `TestVllmClientErrorMapping::test_404_maps_to_model_not_found` |
| generate OOM → `OutOfMemoryError` | `TestVllmClientErrorMapping::test_500_with_oom_message_maps_to_oom` |
| generate malformed response → typed error or documented degradation | `test_generate_malformed_nonstream_response_degrades` (documented degradation, see Known Limitations §2) + `TestVllmStreaming::test_ignores_malformed_data_lines` (stream path resilient-skip) |
| generate_batch preserves input order | `test_generate_batch` (asserts per-index text) |
| generate_batch handles empty/edge intentionally | `test_generate_batch_handles_empty_choices_per_item` |
| embed returns Embedding or raises Unsupported | `test_embed_returns_embedding_dataclass`, `test_embed_distributes_tokens_across_inputs`, `test_embed_falls_back_to_char_estimate_when_usage_absent`, `test_embed_when_uninitialized_raises` |
| health_check healthy/unavailable | `test_health_check_healthy`, `test_health_check_unavailable_when_uninitialized`, `test_health_check_unavailable_when_probe_fails` |
| capabilities truthful per flag | `test_capabilities`, `test_capabilities_reflect_disabled_lora`, plus live `test_capabilities_reflect_live_backend` |
| shutdown closes resources / idempotent | `test_shutdown_idempotent` + live `test_shutdown_idempotent` |
| client/session reuse across ≥2 requests | `test_initialize_reuses_client_across_calls` |

Mapped against §"Integration Mock Test Minimums":

| Required behavior | Test |
|:--|:--|
| successful backend readiness check | `test_initialize` (real HTTP via streaming-server probe is not used for init; mocked path covers the contract because `VllmClient.health` is a single GET) |
| backend not listening / unavailable | `TestVllmClientErrorMapping::test_backend_unreachable_raises_typed_error` (real `urlopen` against closed port 65535 → typed MAI error) |
| one malformed JSON / body / frame | `TestVllmStreaming::test_ignores_malformed_data_lines` (real-HTTP server emits invalid JSON between two valid frames; client parser skips it) |
| one backend-native error response | every `TestVllmClientErrorMapping::*` test |
| one streaming frame sequence + termination | `TestVllmStreaming::test_done_terminator_ends_stream`, `test_yields_tokens_in_order`, `test_synthetic_end_token_on_empty_final_chunk` |
| connection/session reuse across ≥2 requests | `test_initialize_reuses_client_across_calls` |
| cleanup after shutdown | `test_shutdown_idempotent` |

## Unit And Mock Integration Commands

```powershell
cd mai
python -m pytest adapters\vllm\tests -q
```

Last run on this host: `50 passed, 7 skipped in 4.70s` (the 7 skips
are the live tests skipping cleanly when `VLLM_HOST` is unset).

## Live Backend Command

```powershell
$env:VLLM_HOST="http://127.0.0.1:8000"
# Optional model pin (otherwise first /v1/models entry wins):
$env:VLLM_LIVE_MODEL="Qwen/Qwen2.5-0.5B-Instruct"
python -m pytest adapters\vllm\tests\test_integration_live.py -m live_backend -v
```

## Live Backend Result

NOT RUN. No vLLM server is reachable on this development host
(Windows, no NVIDIA GPU runtime). Skip evidence:

```
7 skipped in 4.70s
```

with the reason string `"VLLM_HOST not set or vLLM server unreachable
— set VLLM_HOST=http://127.0.0.1:8000 to enable live tests."` per the
inline `require_live_vllm` autouse fixture.

Convergence-pass and outside reviewer hosts with a real vLLM
deployment must re-run the live command above and record the result
here (or in a sibling evidence file dated with the run). Per
`docs/RC1-TEST-EVIDENCE.md` precedent, the live test must hit a
small embedding-incapable instruct model (e.g.
`Qwen/Qwen2.5-0.5B-Instruct`) for the generate + stream + health
tests, and a separate embedding-model launch (e.g.
`BAAI/bge-small-en-v1.5` with `--task embed`) is required to exercise
`test_embeddings_when_backend_supports_else_unsupported` on the
positive branch. Without the embedding launch, that test skips with
a clean honest reason — not a failure.

## Capability Truth Table

vLLM declarative flags (from `adapter.py:269-285`) vs. implementation:

| Flag | Declared | Implementation evidence | Verdict |
|:--|:-:|:--|:--|
| supports_streaming | True | `_generate_stream` + `client._stream_request` SSE parser + 6 real-HTTP streaming tests | TRUE |
| supports_batching | True | `generate_batch` iterates prompts via `asyncio.to_thread` — not native batching, see Known Limitations §3 | TRUE (qualified) |
| supports_continuous_batching | True | vLLM server-side PagedAttention; adapter does not gate this behavior | TRUE (delegated to backend) |
| supports_structured_output | True | `generate` forwards `guided_json` kwarg; `test_generate_passes_guided_json_for_structured_output` proves the field arrives at the backend | TRUE |
| supports_tool_calling | True | Adapter does NOT forward `tools[]` or `tool_choice` to chat completions. See Known Limitations §4. | DECLARATIVE ONLY |
| supports_embedding | True | `embed()` calls `client.embeddings`, parses `data[*].embedding` into `Embedding` dataclass; 4 unit tests cover the path | TRUE |
| supports_hot_swap | True | `load_lora` / `unload_lora` / `switch_model` exist on adapter and call dedicated client endpoints; 5 unit tests cover both paths | TRUE |
| supports_vision | False | No vision input plumbing | TRUE (correctly absent) |
| max_context_window=32768 | — | Hardcoded; vLLM exposes this per-model via `/v1/models`. See Known Limitations §5. | STALE-RISK |
| backend_version="0.6.0" | — | Hardcoded; not introspected from the live `/version` endpoint. See Known Limitations §5. | STALE-RISK |

## Error Mapping Evidence

All client error paths covered in `TestVllmClientErrorMapping`:

| Backend condition | Mapped MAI error | Test |
|:--|:--|:--|
| 404 (unknown model) | `ModelNotFoundError` | `test_404_maps_to_model_not_found` |
| 429 (rate limited) | `RateLimitedError` | `test_429_maps_to_rate_limited` |
| 408 / 504 (gateway timeout) | `AdapterTimeoutError` | `test_408_maps_to_timeout`, `test_504_maps_to_timeout` |
| 500 + "out of memory" body | `OutOfMemoryError` | `test_500_with_oom_message_maps_to_oom` |
| 400 + "context length" body | `ContextExceededError` | `test_400_with_context_length_message_maps_to_context_exceeded` |
| Generic 5xx | `BackendUnavailableError` | `test_generic_500_maps_to_backend_unavailable`, `test_500_with_malformed_body_falls_through_to_unavailable` |
| Connect-refused / SYN-drop | `BackendUnavailableError` OR `AdapterTimeoutError` (OS-dependent) | `test_backend_unreachable_raises_typed_error` |
| 400 with no mapped detail | Silent (caller's post-handle fallback path covers) | `test_400_without_known_detail_does_not_raise` |
| `URLError("timed out")` from `_request` | `AdapterTimeoutError` (client.py:92-93) | exercised via adapter-layer `test_generate_propagates_client_timeout` |

## Known Limitations

1. **Config validation does not raise `ValidationError`.** `VllmConfig.from_dict`
   silently coerces unknown fields into `extra_options` and never validates
   numeric ranges (e.g. negative `port`, zero `timeout_ms`, fraction
   `gpu_memory_utilization` outside [0.0, 1.0]). The shared contract
   requires "initialize config validation maps to `ValidationError`" as a
   minimum test. Adding validation is an adapter behavior change that
   should be done deliberately under a convergence session — not snuck
   into J-18.

2. **Non-streaming malformed response degrades silently, does not raise.**
   `test_generate_malformed_nonstream_response_degrades` pins the
   adapter's current behavior: a body without `choices` returns an
   empty `GenerationResult` rather than a typed error. The
   `ADAPTER-SHARED-CONTRACT` §"Error Mapping" implies "malformed
   response → typed adapter error" but the matrix §2.3 already classified
   this as "malformed-response partial". Convergence may choose to
   tighten this — either by raising `BackendUnavailableError` or by
   introducing a new `MalformedResponseError` — but the change touches
   `adapter.py` and would shift the contract for callers.

3. **`generate_batch` is sequential, not natively-batched.** The adapter
   iterates over prompts and calls `chat_completions` once per prompt
   via `asyncio.to_thread`. vLLM's native continuous-batching kicks in
   at the server side regardless of how the requests arrive, so the
   throughput claim still holds, but the adapter does not concurrently
   dispatch (i.e. no `asyncio.gather`). Documented here for the
   convergence pass; not in J-18 scope.

4. **`supports_tool_calling=True` is declarative only.** Adapter does
   not forward `tools[]` or `tool_choice` in the chat-completions body
   (`client.chat_completions` does not even accept those kwargs).
   Convergence-pass options: (a) flip the capability to False, or
   (b) extend `chat_completions` and `generate` to plumb tool calls
   through and forward backend `tool_calls` parsing into a structured
   adapter return shape. Either is out of J-18 scope.

5. **`max_context_window` and `backend_version` are hardcoded.** Real
   values vary per model and per vLLM build. Convergence may want to
   introspect via `/v1/models` and `/version` during `initialize` and
   memoize on the adapter instance. Not in J-18 scope.

6. **Test markers `unit` and `integration` are not registered.** Only
   `live_backend` is registered in `mai/conftest.py`. The harness lock
   §"Test Markers" expects `unit`, `integration`, `live_backend`,
   `slow`, `gpu`, `platform_specific`. Registering the additional
   markers is a shared-conftest edit and is deferred to convergence
   so parallel J-19..J-26 sessions do not collide on `conftest.py`.

7. **`vllm_available` fixture is module-local.** Per parallel-merge
   rules, J-18 does not edit `mai/conftest.py`. The inline probe in
   `test_integration_live.py` is functionally equivalent to the
   `ollama_available` / `llamacpp_available` session fixtures already
   in `conftest.py`. Convergence may migrate the body of
   `vllm_available` and `_http_get_*` helpers into `mai/conftest.py`
   for parity if desired.

## Completion Verdict

COMPLETE for J-18 acceptance criteria:

- ✅ embed return-type bug closed (matrix §3 finding #1)
- ✅ unit test surface grew from 16 assertions to 50 tests covering
  every harness §"Unit Test Minimums" item (config-validation gap
  documented under Known Limitations §1)
- ✅ real-HTTP streaming tests via `adapters/tests/_streaming_server`
  exercise the full client + adapter SSE path with no mocks
- ✅ live test file added with inline `VLLM_HOST` probe; skips cleanly
  when env var unset (`7 skipped`)
- ✅ no shared-file edits — `adapters/base.py`, `mai/conftest.py`, and
  `docs/ADAPTER-COMPLETION-MATRIX.md` untouched
- ✅ evidence note (this file) follows the harness lock template

GATED: live backend verification requires a real vLLM deployment.
Convergence pass or outside reviewer must run the Live Backend
Command above and append the result to this file (or a dated sibling
note) before flipping `ADAPTER-COMPLETION-MATRIX.md` §2.3 verdict to
COMPLETE-WITH-LIVE-EVIDENCE.
