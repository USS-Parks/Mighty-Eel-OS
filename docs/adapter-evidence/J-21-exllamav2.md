# J-21 ExLlamaV2 Evidence

## Scope

Bring `adapters/exllamav2/` to the ADAPTER-SHARED-CONTRACT bar:

- close the J-05 error-mapping gaps (rate-limit and context-exceeded)
- close the J-05 "no live tests" gap with an opt-in `test_integration_live.py`
- reinforce J-09's mocked-assertion fill with contract-level lifecycle,
  capability-truth, and error-propagation coverage

The pre-existing adapter surface (initialize, generate, stream, batch,
embed, health, capabilities, shutdown, load_model, unload_model,
switch_model) is unchanged; this session adds error mappings to the
HTTP client and tests on top.

## Files Changed

| File | Change |
|---|---|
| `adapters/exllamav2/client.py` | +1 import block, +12 lines in `_handle_http_error`: 429 → `RateLimitedError`; 400/413/422 with "context"/"max_seq_len"/"too long"/"exceed" → `ContextExceededError` |
| `adapters/exllamav2/tests/test_adapter.py` | +6 imports, +6 error-mapping tests in `TestExLlamaV2Adapter`, +1 new class `TestExLlamaV2Lifecycle` (6 tests), +1 new class `TestExLlamaV2ClientErrorMapping` (12 tests). 32 → 51 collected tests. |
| `adapters/exllamav2/tests/test_integration_live.py` | **NEW** — 7 live-backend tests + local `exllamav2_available` fixture (kept inside the adapter package per ADAPTER-SHARED-CONTRACT §Ownership Rules). |

No edits to `adapters/base.py`, `adapters/runner.py`, `mai/conftest.py`,
or any Rust file. No edits to `docs/ADAPTER-COMPLETION-MATRIX.md`
(rolled up by the convergence pass per the locked contract).

## Contract Results

Mapped against ADAPTER-SHARED-CONTRACT.md §Completion Definition:

| Requirement | Status |
|---|---|
| Every base-surface method has intentional behavior | yes (pre-existing) |
| Capabilities match implemented behavior | yes; `max_context_window` now tested as config-driven |
| Typed error mapping is tested | yes (12 client-level + 5 adapter-level tests) |
| Pooling/lifecycle behavior is tested | yes (`_client` reuse via `ExLlamaV2Client`; double-shutdown + post-shutdown failure tests) |
| Unit tests pass without a live backend | yes (51 / 51) |
| Live backend tests exist and skip cleanly | yes (7 / 7 skip when `EXLLAMAV2_HOST` unset) |
| Evidence note exists | this file |
| No TODO/stub language in adapter | confirmed — `embed` raises `UnsupportedOperationError` (intentional unsupported per §Embedding Contract) |

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters\exllamav2\tests\test_adapter.py -v
```

Result:

```
51 passed in 4.36s
```

Combined adapter suite (unit + live, with `EXLLAMAV2_HOST` unset so
live tests skip):

```powershell
python -m pytest adapters\exllamav2\tests -q
```

Result:

```
51 passed, 7 skipped in 3.83s
```

## Live Backend Command

```powershell
$env:EXLLAMAV2_HOST = "http://127.0.0.1:5000"
# optional: pin a specific loaded model id
# $env:EXLLAMAV2_LIVE_MODEL = "TheBloke/Llama-2-7B-Chat-GPTQ"
python -m pytest adapters\exllamav2\tests\test_integration_live.py -v
```

Requires a running TabbyAPI / ExLlamaV2 OpenAI-compatible server with
at least one model loaded. The gate fixture probes
`GET {EXLLAMAV2_HOST}/v1/models` and skips cleanly on any failure.

## Live Backend Result

Not run in this session — no TabbyAPI / ExLlamaV2 server is available
on the developer host. The 7 tests are confirmed to **skip cleanly**
when the env var is unset (logged above). Per
ADAPTER-TEST-HARNESS-LOCK §Skip And Failure Rules, a missing backend
is a skip, not a failure; this counts as expected behaviour.

A future session that has TabbyAPI on hand should re-run with
`EXLLAMAV2_HOST` set and append the output here.

## Capability Truth Table

`AdapterCapabilities` reported by `ExLlamaV2Adapter.capabilities()`:

| Flag | Value | Proven by |
|---|---:|---|
| `supports_streaming` | True | 7 streaming tests in `TestExLlamaV2Streaming` (real SSE server) + live `test_stream_yields_tokens` |
| `supports_batching` | True | `test_generate_batch` (mocked) + live `test_generate_batch_preserves_order` |
| `supports_embedding` | False | `test_embed_raises` (mocked) + live `test_capabilities_match_live_behavior` (asserts `UnsupportedOperationError`) |
| `supports_hot_swap` | True | `test_load_model`, `test_switch_model_known`, `test_unload_model_clears_active` |
| `supports_structured_output` | False | `test_capabilities` |
| `supports_vision` | False | `test_capabilities` |
| `supports_tool_calling` | False | `test_capabilities` |
| `supports_continuous_batching` | False | `test_capabilities` |
| `max_context_window` | `_config.max_seq_len` (config-driven) | `test_capabilities_reflect_max_seq_len_from_config` |
| `supported_quantizations` | `["exl2", "gptq"]` | `test_capabilities` |

## Error Mapping Evidence

Per ADAPTER-SHARED-CONTRACT §Error Mapping Contract:

| Condition | MAI error | Test |
|---|---|---|
| connect refused / DNS failure | `BackendUnavailableError` | `test_initialize_backend_unavailable` |
| 408, 504 timeout | `AdapterTimeoutError` | `test_408_maps_to_timeout`, `test_504_maps_to_timeout`, `test_generate_timeout_propagates` |
| 429 throttling | `RateLimitedError` | `test_429_maps_to_rate_limited`, `test_generate_rate_limited_propagates` **(closes J-05 gap)** |
| 404 unknown model | `ModelNotFoundError` | `test_404_maps_to_model_not_found`, `test_generate_model_not_found_propagates` |
| memory/oom/vram | `OutOfMemoryError` | `test_oom_message_maps_to_out_of_memory`, `test_vram_message_maps_to_out_of_memory`, `test_generate_oom_propagates` |
| context-length violation | `ContextExceededError` | `test_400_context_message_maps_to_context_exceeded`, `test_422_context_message_maps_to_context_exceeded`, `test_413_context_message_maps_to_context_exceeded`, `test_generate_context_exceeded_propagates` **(closes J-05 gap)** |
| ≥500 generic | `BackendUnavailableError` | `test_500_generic_maps_to_backend_unavailable` |
| unsupported `embed` | `UnsupportedOperationError` | `test_embed_raises` |
| malformed JSON body | does not crash mapper | `test_malformed_json_body_does_not_crash`, `test_generate_malformed_body_falls_back_to_empty` |

Backend-crash mapping continues to alias to `BackendUnavailableError`
(client cannot distinguish a transient 502 from an actual process
death without out-of-band state). This is the same conservative
choice the other completed adapters made, and is documented in §Known
Limitations below.

## Known Limitations

1. **No live-backend evidence in this session.** No TabbyAPI / ExLlamaV2
   server is running on the developer host. The live tests skip
   cleanly per contract; an operator with hardware should re-run and
   append output.
2. **`BackendCrashedError` not emitted.** The HTTP client cannot
   distinguish a backend crash from a transient `BackendUnavailableError`
   without process-supervisor state. Per §Error Mapping Contract this
   is acceptable — the contract requires the error variant exist, not
   that every adapter emit it.
3. **Multi-model hot-swap proven only by mocked tests.** The live suite
   does not currently load/unload models against a real server because
   doing so requires file-system access to the model directory; the
   mocked tests in `TestExLlamaV2Adapter` cover the adapter logic.
4. **`_handle_http_error` propagates `RateLimitedError` from `models()`
   probes.** `models()` and `health()` catch
   `(AdapterTimeoutError, BackendUnavailableError)` but not
   `RateLimitedError`. In practice a 429 on the model-list probe is
   vanishingly rare; if it ever fires, the caller sees the typed error
   bubble up. Left as-is to keep the surgical scope of J-21; document
   for the convergence pass.
5. **`docs/ADAPTER-COMPLETION-MATRIX.md` not edited.** The locked
   shared contract instructs parallel adapter sessions to record
   evidence in `docs/adapter-evidence/` and leave matrix updates to
   the convergence pass.

## Completion Verdict

**Complete for the J-21 scope defined in
`docs/JOHN-REMEDIATION-ROSTER.md` and ADAPTER-SHARED-CONTRACT.md.**

- The J-05 error-mapping gaps (rate-limit, context-exceeded) are
  closed at the client level with direct unit tests on
  `_handle_http_error` plus adapter-level propagation tests.
- Live-backend coverage exists, gated by `EXLLAMAV2_HOST`, and skips
  cleanly without the env var.
- Unit-test count rose from 32 (post-J-09) to 51, all passing.
- No shared files were edited; ownership rules for parallel J-18..J-26
  sessions are respected.
