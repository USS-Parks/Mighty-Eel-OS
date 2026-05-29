# GitDoctor 75 Adapter Timeouts + Pooling Evidence (GD75-07)

**Date:** 2026-05-25  
**Goal:** document (and where necessary, tighten) timeout defaults and connection pooling across adapters without changing MAI’s air-gapped localhost model.

## Summary

- Adapters that speak HTTP use stdlib `urllib` clients with a **persistent opener per client instance** (connection pooling reuse).
- Each HTTP client carries **separate unary vs streaming timeouts** (`timeout_ms`, `stream_timeout_ms`).
- Health/info/metrics probes now use the configured `health_check_timeout_ms` instead of a hard-coded constant.

## Representative Implementations

- Pooling via persistent opener:
  - `adapters/openai_compat/client.py`
  - `adapters/tensorrt/client.py`
  - `adapters/triton/client.py`
- Configured timeouts (unary/stream/health):
  - `adapters/vllm/client.py`
  - `adapters/sglang/client.py`
  - `adapters/tgi/client.py`

## Test Evidence

- vLLM / SGLang / TGI adapter tests include timeout + pooling expectations:
  - `adapters/vllm/tests/test_adapter.py`
  - `adapters/sglang/tests/test_adapter.py`
  - `adapters/tgi/tests/test_adapter.py`

