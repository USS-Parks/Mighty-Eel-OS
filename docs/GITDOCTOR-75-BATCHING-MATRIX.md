# GitDoctor 75 Batch Capability Matrix (GD75-10)

**Date:** 2026-05-25  
**Goal:** make batching support explicit per adapter/scheduler without adding cloud assumptions.

## Scheduler batching primitives

- Continuous batching subsystem exists in `mai-scheduler/src/batch/`.
- Adapter IPC supports `generate_batch` via `mai-adapters/src/manager.rs`.

## Adapter capability matrix (high-level)

| Adapter | `supports_batching` | `supports_continuous_batching` | Implementation note |
|---|---:|---:|---|
| `vllm` | yes | yes | server-side continuous batching (PagedAttention); adapter exposes `generate_batch` |
| `tgi` | yes | yes | server-side continuous batching; adapter exposes `generate_batch` |
| `tensorrt` | yes | config-gated | batch + optional in-flight batching (config) |
| `sglang` | yes | yes | server-side batching; adapter exposes batch surface |
| `mlx` | yes | no | bounded adapter-level fan-out (no native batch API) |
| `exllamav2` | yes | no | bounded batch fan-out; `max_batch_size` config |
| `triton` | conditional | no | batching depends on operator tensor wiring (`declares_batching`) |
| `ollama` | no | no | sequential only |
| `openai_compat` | no | no | sequential only |
| `llamacpp` | no | no | sequential only |
| `onnxruntime` | no | no | sequential only (no native batch surface) |

## GD75-10 follow-on (implementation guidance)

Prefer batching where it is already natively supported (vLLM/TGI/SGLang/TensorRT) and keep "adapter-level fan-out" bounded for backends that lack native batching (MLX/ExLlamaV2). Avoid adding cross-request response caching as a "batching substitute".

