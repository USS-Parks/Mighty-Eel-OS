# GitDoctor 75 Response Caching Policy (GD75-14)

**Date:** 2026-05-25  
**Goal:** respond to “missing caching” narrative findings without introducing unsafe cross-request data leakage or cloud assumptions.

## Policy

- **Inference responses are not cached by default.** Prompt content can be sensitive and caching adds data-retention risk.
- **If caching is enabled**, it must be:
  - **opt-in** (explicit configuration)
  - **bounded** (TTL + memory budget)
  - **isolated** (per-profile / per-auth context)
  - **content-addressed** (hashed keys; no prompt text in cache keys)
- **Non-sensitive caches are allowed** where they do not store user payloads (e.g., scheduler decision cache, model metadata caches).

## Existing Capability (evidence)

- A bounded, TTL + LRU response-cache implementation exists in:
  - `mai-core/src/cache.rs`

## Rationale (air-gapped appliance)

MAI’s primary deployment target is a single-node, localhost-bound, air-gapped appliance. A generic “add response caching” recommendation from a web-service scanner is not automatically a production improvement here; the default posture is to avoid storing user prompts/responses beyond the audit/logging policy.

