# GitDoctor 75 Adapter Client Lifecycle (GD75-08)

**Date:** 2026-05-25  
**Goal:** prove adapters reuse pooled HTTP clients and release them cleanly on shutdown (idempotent), to support production-readiness narrative without adding cloud assumptions.

## What Changed

- Added explicit pooled-connection lifecycle to the stdlib-urllib HTTP clients:
  - `adapters/vllm/client.py` now owns a persistent opener (`client.opener`) and exposes `close()`.
  - `adapters/sglang/client.py` now owns a persistent opener (`client.opener`) and exposes `close()`.
  - `adapters/tgi/client.py` now owns a persistent opener (`client.opener`) and exposes `close()`.
- Updated adapters to call client `close()` during `shutdown()` (tolerates sync or AsyncMock via `maybe_await`):
  - `adapters/vllm/adapter.py`
  - `adapters/sglang/adapter.py`
  - `adapters/tgi/adapter.py`

## Test Evidence

- vLLM client pooling + close behavior is pinned:
  - `adapters/vllm/tests/test_adapter.py`
- Adapter shutdown/idempotency tests continue to pass:
  - `adapters/sglang/tests/test_adapter.py`
  - `adapters/tgi/tests/test_adapter.py`

