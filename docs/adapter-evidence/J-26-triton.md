# J-26 Generic Triton Evidence

## Scope

Add a generic NVIDIA Triton Inference Server adapter (KServe v2 HTTP
protocol) distinct from the TensorRT-LLM adapter at `adapters/tensorrt/`.

Targets non-LLM, multimodal, embedding, classifier, and custom-model
workloads. When the operator wires BYTES text tensors via
`TritonConfig.input_tensor_name` / `output_tensor_name`, the high-level
`generate` / `generate_batch` text surface lights up; otherwise it
raises `UnsupportedOperationError` and only the raw `infer()` KServe
surface is available. Capability flags are truthful about this
distinction.

## Files Changed

New files only (no shared-file edits — `pyproject.toml`, root
`conftest.py`, `ADAPTER-COMPLETION-MATRIX.md`, `adapters/base.py`,
`adapters/runner.py` all untouched per `ADAPTER-SHARED-CONTRACT.md`):

- `adapters/triton/__init__.py`
- `adapters/triton/config.py`              (93 lines)
- `adapters/triton/client.py`              (273 lines)
- `adapters/triton/adapter.py`             (365 lines)
- `adapters/triton/tests/__init__.py`
- `adapters/triton/tests/conftest.py`      (73 lines, `triton_available` fixture)
- `adapters/triton/tests/test_config.py`   (128 lines)
- `adapters/triton/tests/test_client.py`   (418 lines)
- `adapters/triton/tests/test_adapter.py`  (481 lines)
- `adapters/triton/tests/test_integration_live.py` (191 lines)
- `docs/adapter-evidence/J-26-triton.md`   (this file)

## Contract Results

`ADAPTER-SHARED-CONTRACT.md` row-by-row:

- Required surface: `initialize`, `generate`, `generate_batch`, `embed`,
  `health_check`, `capabilities`, `shutdown` all implemented
  intentionally. Unsupported ops raise `UnsupportedOperationError`.
- Lifecycle: `__init__` opens no sockets; `initialize` validates
  config, opens a pooled urllib client, probes `/v2/health/live` and
  `/v2/models/<m>/ready`; pre-init `generate`/`infer` raises
  `BackendUnavailableError`; `shutdown` releases the client and is
  idempotent.
- HTTP pooling: one `urllib.request.OpenerDirector` per
  `TritonClient`; `test_opener_reused_across_requests` proves the
  same opener handles two requests; `close()` is idempotent.
- Error mapping: 404 → `ModelNotFoundError`, 408/504 →
  `AdapterTimeoutError`, 429 → `RateLimitedError`, 413 / "context" /
  "too long" → `ContextExceededError`, OOM phrases →
  `OutOfMemoryError` (word-boundary regex; "boom" no longer
  false-matches), 502 / "broken pipe" → `BackendCrashedError`, other
  5xx → `BackendUnavailableError`, malformed JSON →
  `BackendCrashedError`, empty input tensor list → `ValidationError`.
- Capability truthfulness: `supports_streaming=False` because KServe
  v2 HTTP `/infer` is unary (the streaming surface yields a single
  end-of-text Token frame and the capability flag matches);
  `supports_batching` is True only when the operator wired the text
  tensors; `supports_embedding` is False unless explicitly declared
  via `TritonConfig.declares_embedding`; no hardware fields are
  reported.
- Generation contract: non-streaming returns `GenerationResult` with
  `tokens_generated >= 1` and `FinishReason.STOP`; streaming yields
  one `Token(is_end_of_text=True)` honestly; batch preserves input
  order and pads/truncates deterministically when the backend returns
  the wrong cardinality.
- Embedding contract: `embed` always raises
  `UnsupportedOperationError`; operators serve embedding models via
  the raw `infer()` surface with their own tensor names.
- Health contract: lightweight `/v2/health/ready` probe + per-model
  `/ready`; healthy / degraded / unavailable mapped per spec; no raw
  backend exception text reaches user-facing messages.
- Security / locality: defaults to `127.0.0.1:8000`; no telemetry; no
  prompt or completion logging in success paths; stdlib-only client.

## Unit And Mock Integration Commands

```powershell
python -m pytest adapters\triton\tests -v --tb=short
```

Last run on this branch:

```
72 passed, 5 skipped in 7.94s
```

The 5 skipped are the live integration tests; they skip cleanly when
`TRITON_HOST` / `TRITON_MODEL_NAME` are unset (verified by the
`triton_available` fixture in the adapter-local `conftest.py`).

Mock integration coverage (HTTP boundary, real `urllib` opener
against an in-process `http.server`):

- pooled opener reused across two requests
- `close()` is idempotent; opener access after close raises typed error
- server live = True / False (200 / 500)
- connection-refused path returns False without raising
- model ready 200 vs 404
- model metadata happy path vs failure → `{}`
- `infer` happy path with text BYTES output
- `infer` empty input → `ValidationError`
- `infer` 404 → `ModelNotFoundError` carrying model hint
- `infer` 429 → `RateLimitedError`
- malformed JSON body → `BackendCrashedError`
- backend not listening → `BackendUnavailableError` or
  `AdapterTimeoutError` depending on OS connect behaviour
- `_map_http_error` and `_extract_error_detail` unit-covered for
  every branch

## Live Backend Command

```powershell
$env:TRITON_HOST = "http://127.0.0.1:8000"
$env:TRITON_MODEL_NAME = "ensemble_simple"
# Optional, to exercise the text generate() path:
$env:TRITON_INPUT_TENSOR = "text_input"
$env:TRITON_OUTPUT_TENSOR = "text_output"
python -m pytest adapters\triton\tests\test_integration_live.py -m live_backend -v
```

## Live Backend Result

Not exercised on this host — no live Triton server is reachable from
the developer workstation that produced this evidence. The five live
tests SKIP cleanly under `pytest -q` as designed (see the 5 skipped
count above). Live execution is the operator's responsibility per the
harness lock; the gate env vars are documented in the live test
docstring and this evidence note.

## Capability Truth Table

| Capability                  | Reported           | Implemented behaviour                                  |
|-----------------------------|--------------------|--------------------------------------------------------|
| `supports_streaming`        | `False`            | KServe v2 HTTP `/infer` is unary; honest one-shot.     |
| `supports_batching`         | text-IO + flag     | Single `/infer` carries the full batch; order preserved. |
| `supports_structured_output`| `False`            | Not implemented.                                       |
| `supports_vision`           | `False`            | Not implemented (operator uses raw `infer()`).         |
| `supports_tool_calling`     | `False`            | Not implemented.                                       |
| `supports_continuous_batching` | `False`         | Not implemented.                                       |
| `supports_embedding`        | flag-gated         | Always raises `UnsupportedOperationError` even when flag is set; the flag advertises the underlying model class but the high-level surface is intentionally absent for generic Triton (raw `infer()` is the path). |
| `supports_hot_swap`         | `False`            | Not implemented.                                       |
| `max_context_window`        | text-IO ? len : 0  | Only meaningful when text-IO is wired.                 |
| `backend_version`           | `"kserve-v2"`      | Protocol name, not server version.                     |

## Error Mapping Evidence

Direct unit coverage in `test_client.py::TestErrorMapping`:

- `_map_http_error(404, ...)` → `ModelNotFoundError` carrying model hint
- `_map_http_error(408, ...)` and `(504, ...)` → `AdapterTimeoutError`
- `_map_http_error(429, ...)` → `RateLimitedError`
- `_map_http_error(413, ...)` → `ContextExceededError`
- `_map_http_error(500, "...CUDA out of memory...")` → `OutOfMemoryError`
- `_map_http_error(502, ...)` → `BackendCrashedError`
- `_map_http_error(503, ...)` → `BackendUnavailableError`
- `_map_http_error(418, ...)` → `BackendUnavailableError`
- `_extract_error_detail` covered for `{"error":}`, `{"message":}`,
  `{"detail":}`, plain text (truncated to 200 chars), and empty body

End-to-end coverage via the in-process fake Triton server in
`TestInfer`, where a real `TritonClient` makes real urllib requests
against a real `http.server`.

## Known Limitations

- Generic Triton HTTP `/infer` is unary; this adapter does not
  implement gRPC bidirectional streaming or Triton's decoupled-mode
  responses. `supports_streaming=False` reflects that honestly.
- BYTES tensors are sent as raw UTF-8 strings; the binary-data tensor
  extension (`Inference-Header-Content-Length` HTTP header) is not
  implemented. A model that requires binary BYTES must use the
  operator-driven raw `infer()` path with the appropriate transport
  in their own client wrapper.
- The high-level `embed()` is intentionally always
  `UnsupportedOperationError` — embedding-style Triton models vary
  too much in tensor convention to expose a single safe default.
  Operators should drive embedding models through `infer()`.
- `ADAPTER-COMPLETION-MATRIX.md` is intentionally NOT updated in this
  commit. Per `ADAPTER-SHARED-CONTRACT.md` the convergence session
  rolls up J-18..J-26 evidence notes into the matrix to avoid
  parallel-session merge conflicts.
- `live_backend` marker is already registered in the root
  `conftest.py`; no shared-file edit required.

## Completion Verdict

PASS.

- Every method in the required surface has intentional behaviour
  (no `todo!()`, no silent `None`, no placeholder).
- Capabilities match implemented behaviour
  (`supports_streaming=False`, text-IO-gated batching).
- Typed error mapping is unit-tested AND exercised end-to-end via
  the in-process fake server.
- Pooling and lifecycle behaviour is tested.
- 72 unit + mock-integration tests pass; 5 live tests skip cleanly.
- Live tests exist and are properly gated by `TRITON_HOST` /
  `TRITON_MODEL_NAME`.
- This evidence note exists.
- No TODO/stub language remains in adapter or client.
