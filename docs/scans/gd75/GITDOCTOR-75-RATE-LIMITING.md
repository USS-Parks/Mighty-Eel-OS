# GitDoctor 75 Rate Limiting Evidence (GD75-12)

**Date:** 2026-05-25  
**Goal:** demonstrate rate limiting exists, is tested, and returns scanner-recognizable evidence (429 + Retry-After).

## Behavior

- The auth middleware enforces per-key request limits and returns:
  - HTTP `429 Too Many Requests`
  - MAI error code `MAI-4005`
  - `Retry-After` header (seconds)

## Implementation

- Rate limiter + middleware path:
  - `mai-api/src/auth.rs`
- Error shape + Retry-After header wiring:
  - `mai-api/src/errors.rs`
- Metrics counter for rate limiting:
  - `mai-api/src/metrics.rs` (`mai_rate_limited_total`)

## Test Evidence

- Integration test proving rate-limit returns 429:
  - `mai-api/tests/auth_gate_a.rs`
- Observability test asserts rate-limited metric is emitted:
  - `mai-api/tests/ship_11_observability.rs`

