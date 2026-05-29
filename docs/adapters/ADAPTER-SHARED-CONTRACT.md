# MAI Shared Adapter Contract

Status: locked for J-18 through J-26 parallel adapter completion
Owner: W3 adapter convergence lane
Last updated: 2026-05-24

This file is the shared contract for all adapter completion sessions. Every
J-18 through J-26 session must read this file before editing adapter code.
Adapter-specific work may differ by backend, but the surface below must not
drift unless a separate convergence session updates this contract first.

## Applies To

| Session | Adapter |
|---|---|
| J-18 | vLLM |
| J-19 | TGI |
| J-20 | SGLang |
| J-21 | ExLlamaV2 |
| J-22 | TensorRT-LLM/Triton |
| J-23 | Generic OpenAI-compatible local |
| J-24 | ONNX Runtime |
| J-25 | MLX |
| J-26 | Generic Triton |

Ollama and llama.cpp are the reference adapters. Use their passing behavior
as examples, but do not copy quirks that conflict with this contract.

## Ownership Rules For Parallel Sessions

Each adapter session owns only:

- its adapter package under `adapters/<adapter_name>/`
- its adapter-specific unit tests
- its adapter-specific opt-in live tests
- an adapter-specific evidence note under `docs/adapter-evidence/`

Do not edit `adapters/base.py`, `adapters/runner.py`, shared Rust adapter
manager files, or shared test fixtures from J-18 through J-26 unless the
session prompt explicitly grants shared-file ownership. If a shared contract
gap is discovered, document it in the adapter evidence note and leave the
shared change for the convergence pass.

To avoid merge conflicts, parallel sessions should not all edit
`docs/ADAPTER-COMPLETION-MATRIX.md`. Record completion evidence in
`docs/adapter-evidence/J-XX-<adapter>.md`; the convergence session rolls
those notes into the matrix.

## Required Python Adapter Surface

Every adapter must inherit `AdapterBase` and implement the current base
surface exactly:

- `async initialize(config: dict | None = None, hil_handle: Any | None = None) -> str | None`
- `async generate(prompt: str, params: GenerationParams, *, stream: bool = False) -> GenerationResult | AsyncIterator[Token]`
- `async generate_batch(prompts: list[str], params: GenerationParams) -> list[GenerationResult]`
- `async embed(texts: list[str]) -> list[Embedding]`
- `async health_check() -> HealthStatus`
- `capabilities() -> AdapterCapabilities`
- `async shutdown() -> None`

Method completeness means every method is implemented intentionally. A method
that a backend cannot support must raise `UnsupportedOperationError` with the
operation name; it must not be a placeholder, silent `None`, broad exception,
or fake success.

## Lifecycle Contract

Adapters must have a clear lifecycle:

1. `__init__` stores configuration only. It must not open sockets, start
   servers, load large models, or perform network calls.
2. `initialize` validates config, creates clients or sessions, checks backend
   readiness when a backend is required, and returns a stable handle string
   when available.
3. `generate`, `generate_batch`, `embed`, and `health_check` must fail with a
   typed adapter error if called before successful initialization.
4. `shutdown` closes HTTP clients, sessions, subprocesses, file handles,
   temporary directories, and backend handles. It must be idempotent.
5. If an adapter adds `__aenter__` and `__aexit__`, they must delegate to
   `initialize` and `shutdown` without changing the method contract above.

The session is complete only when unit tests prove initialize, reuse, shutdown,
and double-shutdown behavior.

## HTTP And Session Pooling

HTTP or gRPC clients must be reused for the adapter lifetime. Do not create a
new network connection for every request.

Required behavior:

- one client/session pool per initialized adapter instance
- configurable connect/read/request timeout values
- streaming timeout separate from non-streaming request timeout
- shutdown closes the client/session pool
- tests prove at least two calls reuse the same client/session object or pool

The project currently favors stdlib-only adapter clients where possible.
If an adapter needs a third-party runtime dependency, pin it and document why
the backend cannot be implemented safely with existing project dependencies.

## Error Mapping Contract

Adapters must map backend failures into MAI typed errors. Do not leak raw
backend exceptions or backend-specific response bodies across the adapter
boundary.

| Condition | Required MAI error |
|---|---|
| connect refused, DNS/socket failure, backend not listening | `BackendUnavailableError` |
| deadline exceeded, read timeout, stream timeout | `AdapterTimeoutError` |
| backend process terminated or unusable after prior success | `BackendCrashedError` |
| model missing, not loaded, or unknown model id | `ModelNotFoundError` |
| CUDA/VRAM/host memory exhaustion | `OutOfMemoryError` |
| prompt exceeds model or backend context | `ContextExceededError` |
| 429 or backend throttling | `RateLimitedError` |
| unsupported operation for this backend | `UnsupportedOperationError` |
| invalid user config, schema, path, or provider selection | `ValidationError` |

Tests must cover at least unavailable backend, timeout, model missing when the
backend exposes model identity, unsupported embedding when applicable, and one
backend-specific error that maps to a typed MAI error.

## Capability Truthfulness

`capabilities()` must report what the adapter actually supports through the
implemented code path, not what the upstream backend might support in theory.

Rules:

- `supports_streaming` is true only if `generate(..., stream=True)` returns a
  real async token iterator or a documented single-event stream wrapper.
- `supports_batching` is true only if `generate_batch` provides native or
  deliberate bounded parallel behavior and tests prove ordering.
- `supports_embedding` is true only if `embed` returns `Embedding` instances
  from a real endpoint/session path.
- `supports_structured_output` and `supports_tool_calling` are true only when
  the adapter sends and parses those backend fields.
- hardware details, VRAM, measured latency, and accelerator state do not
  belong in `AdapterCapabilities`; those remain HIL/manager concerns.

If support is config-dependent, the adapter must report the initialized
capability state and tests must cover both enabled and disabled cases.

## Generation And Streaming Contract

Non-streaming generation must return `GenerationResult` with:

- generated text
- `tokens_generated`
- a valid `FinishReason`

Streaming generation must:

- yield `Token` objects in order
- preserve token text exactly as emitted by the backend
- mark end-of-text consistently
- stop on backend end markers without hanging
- map malformed stream frames into typed errors
- close the stream response on cancellation or shutdown

Do not implement streaming by buffering the whole response unless the adapter
explicitly documents a backend limitation and reports capabilities honestly.

## Batch Contract

`generate_batch` must:

- preserve input order in output order
- validate empty prompt lists
- avoid unbounded task creation
- apply request timeouts
- map per-request failures deterministically

Native backend batching is preferred. If unavailable, bounded adapter-level
parallelism is acceptable and must be documented in the evidence note.

## Embedding Contract

Adapters with embedding support must return `list[Embedding]`, not raw lists.
Each embedding must include:

- `vector: list[float]`
- `input_tokens: int`

Adapters without embedding support must raise `UnsupportedOperationError`.
They must not return empty vectors or fake embeddings.

## Health Contract

`health_check()` must be lightweight and safe under load. It should not run a
full generation request unless the backend has no cheaper readiness primitive.

Required status behavior:

- healthy when the initialized backend is ready for requests
- degraded when the backend is reachable but not fully ready
- unavailable when the backend cannot be reached
- no raw backend exception text in user-facing health messages

## Security And Locality

Adapters must preserve the MAI trust boundary:

- default endpoints must be loopback or local file/session paths
- no telemetry or outbound calls unless the backend requires an explicit local
  server configured by the operator
- no environment secret logging
- no prompt/completion logging in normal success paths
- subprocesses and temp files must be cleaned up during shutdown

## Completion Definition

An adapter session is not complete until all of these are true:

- every method in the required surface has intentional behavior
- capabilities match implemented behavior
- typed error mapping is tested
- pooling/lifecycle behavior is tested
- unit tests pass without a live backend
- live backend tests exist, are marked `live_backend`, and skip cleanly when
  required environment variables are absent
- an evidence note exists under `docs/adapter-evidence/`
- no TODO/stub language remains in the adapter implementation unless it is a
  documented unsupported feature that raises `UnsupportedOperationError`
