# GitDoctor 75 Adapter Input Validation (GD75-05)

**Date:** 2026-05-25  
**Goal:** ensure malformed requests fail *before* any backend call, and provide scanner-friendly evidence that adapters validate inputs.

## What Changed

- Added shared request validation helpers on `AdapterBase`:
  - `AdapterBase._validate_generate_request(...)`
  - `AdapterBase._validate_embed_request(...)`
- Wired validation into multiple adapter families (representative coverage):
  - `adapters/tensorrt/adapter.py`
  - `adapters/openai_compat/adapter.py`
  - `adapters/ollama/adapter.py`
  - `adapters/triton/adapter.py`
  - `adapters/onnxruntime/adapter.py`
  - plus additional adapters with the same call pattern (vLLM, sglang, mlx, exllamav2, tgi, llamacpp).

## Test Evidence (examples)

- TensorRT adapter rejects empty prompt / invalid params:
  - `adapters/tensorrt/tests/test_adapter.py`
- OpenAI-compat adapter rejects invalid input **without** invoking inference endpoints:
  - `adapters/openai_compat/tests/test_adapter.py`
- Ollama adapter rejects empty prompt:
  - `adapters/ollama/tests/test_adapter.py`

