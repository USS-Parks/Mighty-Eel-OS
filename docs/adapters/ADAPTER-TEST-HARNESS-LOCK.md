# MAI Adapter Test Harness Lock

Status: locked for J-18 through J-26 parallel adapter completion
Owner: W3 adapter convergence lane
Last updated: 2026-05-24

This file defines the common test harness expectations for the adapter
completion sessions. The goal is comparable evidence across all adapters, not
nine different private definitions of "done."

## Required Test Layers

Every adapter completion session must provide these layers:

| Layer | Required | Purpose |
|---|---:|---|
| Unit tests | yes | Prove adapter behavior without a live backend |
| Contract tests | yes | Prove shared `AdapterBase` behavior, types, errors, and lifecycle |
| Integration mock tests | yes | Prove HTTP/gRPC/IPC parsing against deterministic local fakes |
| Live backend tests | yes, opt-in | Prove real request/response cycle when backend env vars are supplied |
| Evidence note | yes | Capture commands, skips, backend versions, and limitations |

Live backend tests must skip cleanly by default. A missing backend, model, GPU,
or platform is a skip, not a failure. A supplied backend that returns bad data
is a failure.

## Test Markers

Use these markers consistently:

- `unit`: pure unit tests, no network, no subprocess backend
- `integration`: deterministic local fake server/session tests
- `live_backend`: real external backend or hardware-backed runtime
- `slow`: expected runtime above 10 seconds
- `gpu`: requires CUDA, Metal, or other accelerator hardware
- `platform_specific`: requires macOS, Linux, Windows, or CPU architecture

If the marker configuration is missing, add it in the smallest shared test
config file already used by the repo. Do not create adapter-local marker names
that mean the same thing.

## Live Backend Environment Variables

| Session | Adapter | Live-test gate |
|---|---|---|
| J-18 | vLLM | `VLLM_HOST` |
| J-19 | TGI | `TGI_HOST` |
| J-20 | SGLang | `SGLANG_HOST` |
| J-21 | ExLlamaV2 | `EXLLAMAV2_HOST` or documented in-process model path |
| J-22 | TensorRT-LLM/Triton | `TENSORRT_HOST` or `TRITON_TENSORRT_HOST` |
| J-23 | Generic OpenAI-compatible local | `OPENAI_COMPAT_HOST` |
| J-24 | ONNX Runtime | `ONNXRUNTIME_MODEL_PATH` |
| J-25 | MLX | `MLX_MODEL_PATH` plus Apple Silicon platform check |
| J-26 | Generic Triton | `TRITON_HOST` |

Live tests must also accept adapter-specific model variables when needed, such
as `<ADAPTER>_MODEL`, but the host/path variable above is the primary gate.

## Unit Test Minimums

Each adapter unit suite must cover at least:

- construction stores config without network calls
- initialize happy path
- initialize unavailable backend maps to `BackendUnavailableError`
- initialize config validation maps to `ValidationError`
- generate non-streaming happy path returns `GenerationResult`
- generate streaming happy path yields ordered `Token` objects
- generate timeout maps to `AdapterTimeoutError`
- generate model-not-found maps to `ModelNotFoundError` when supported
- generate backend memory failure maps to `OutOfMemoryError` when detectable
- generate malformed backend response maps to a typed adapter error
- generate_batch preserves input order
- generate_batch handles empty input intentionally
- embed returns `Embedding` objects or raises `UnsupportedOperationError`
- health_check returns healthy/degraded/unavailable as appropriate
- capabilities are truthful for every supported feature flag
- shutdown closes resources
- double shutdown is safe
- post-shutdown calls fail deterministically or reinitialize intentionally

The suite must contain meaningful assertions about returned values and errors.
Smoke-only tests and `assert True` do not count.

## Integration Mock Test Minimums

HTTP/gRPC adapters must use deterministic local fake servers or fake session
objects. In-process runtimes must use deterministic fake modules or tiny model
fixtures.

Mock integration tests must cover:

- successful backend readiness check
- backend not listening or unavailable
- one malformed JSON/body/frame case
- one backend-native error response
- one streaming frame sequence, including termination
- connection/session reuse across at least two requests
- cleanup after shutdown

Tests must not call public internet services.

## Live Backend Test Minimums

Live backend tests must be small and deterministic:

- skip if the gate env var is absent
- skip with a clear reason when platform or hardware is absent
- use a small prompt such as `Say OK.`
- request a tiny token budget
- assert non-empty text or expected structured output
- run one health/readiness probe
- run one generation request
- run one streaming request when the adapter claims streaming
- run one embedding request when the adapter claims embeddings
- assert the adapter reports truthful capabilities for the live backend

Live tests should not download models automatically. The operator supplies the
backend and model through env vars.

## Evidence Note Format

Each session must create:

`docs/adapter-evidence/J-XX-<adapter>.md`

Use this shape:

```markdown
# J-XX <Adapter> Evidence

## Scope

## Files Changed

## Contract Results

## Unit And Mock Integration Commands

## Live Backend Command

## Live Backend Result

## Capability Truth Table

## Error Mapping Evidence

## Known Limitations

## Completion Verdict
```

The evidence note is the parallel-safe replacement for editing the shared
completion matrix during J-18 through J-26.

## Command Expectations

Adapter sessions should run the narrowest useful tests first, then broader
checks if time allows.

Required before commit:

```powershell
python -m pytest adapters\<adapter>\tests -q
```

Required when live backend env vars are supplied:

```powershell
python -m pytest adapters\<adapter>\tests -m live_backend -q
```

Recommended after adapter changes:

```powershell
cargo check --workspace
python tools\local_gitdoctor_scan.py --root . --format json --output docs\LOCAL-GITDOCTOR-REPORT.json
```

Do not mark a session complete if unit tests only pass by mocking the adapter
method under test itself. Mock the backend boundary, not the adapter behavior.

## Skip And Failure Rules

Allowed skips:

- missing live backend gate env var
- missing optional platform or hardware for a live test
- backend feature honestly unsupported and covered by capability flags

Failures:

- live gate env var is supplied but test cannot connect
- adapter returns raw backend exceptions
- adapter reports a capability that the test cannot exercise
- unsupported operation returns fake success
- resource cleanup leaves an open client/session/process
- tests pass without assertions about returned values or typed errors

## Parallel Merge Rules

During J-18 through J-26:

- avoid shared-file edits unless explicitly assigned
- keep tests inside the adapter-owned tree where possible
- write adapter evidence notes instead of editing the completion matrix
- do not change marker semantics for other sessions
- do not relax shared expectations to make one backend pass

The convergence session owns shared rollup, matrix updates, and any necessary
contract revisions after all adapter evidence notes land.
