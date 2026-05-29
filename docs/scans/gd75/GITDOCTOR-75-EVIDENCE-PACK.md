# GitDoctor 75 Evidence Pack (GD75-15)

**Date:** 2026-05-25  
**Audience:** external reviewers, acquirer diligence, internal rescan operators.

This index is the “single page” entry point for the artifacts created during the GitDoctor 75 remediation lane.

## Hard Static Failures (PDF hard fails)

- Baseline reconciliation table: `docs/GITDOCTOR-75-BASELINE.md`
- Lock policy + verification:
  - `docs/DEPENDENCY-LOCK-POLICY.md`
  - `docs/GITDOCTOR-75-LOCK-VERIFICATION.md`
- Integration/e2e manifest:
  - `docs/GITDOCTOR-75-INTEGRATION-MANIFEST.md`
- Assertion audit (scanner parity for TST-004):
  - `docs/GITDOCTOR-75-ASSERTION-AUDIT.md`

## Adapter Safety (validation + error redaction)

- Adapter input validation evidence:
  - `docs/GITDOCTOR-75-ADAPTER-VALIDATION.md`
- Public error redaction evidence:
  - `docs/GITDOCTOR-75-ERROR-REDACTION.md`

## Production API Evidence (health, metrics, rate limiting, OpenAPI)

- Health + metrics evidence:
  - `docs/GITDOCTOR-75-HEALTH-METRICS.md`
- Rate limiting evidence:
  - `docs/GITDOCTOR-75-RATE-LIMITING.md`
- OpenAPI contract (machine-readable):
  - `docs/api/openapi.yaml`

## Performance / Lifecycle Evidence (timeouts, pooling, lifecycle)

- Timeouts + pooling evidence:
  - `docs/GITDOCTOR-75-ADAPTER-TIMEOUTS-POOLING.md`
- HTTP client lifecycle evidence:
  - `docs/GITDOCTOR-75-ADAPTER-LIFECYCLE.md`

## Local Load Balancing + Batching

- Local adapter load-balancing design:
  - `docs/GITDOCTOR-75-LOCAL-ADAPTER-BALANCING.md`
- Batch capability matrix:
  - `docs/GITDOCTOR-75-BATCHING-MATRIX.md`

## Caching Policy

- Response caching decision:
  - `docs/GITDOCTOR-75-CACHING-POLICY.md`

## Contributor Entry Map

- "First 30 minutes" map:
  - `docs/CONTRIBUTOR-ENTRY-MAP.md`

## Suggested Local Commands (offline)

- Rust offline cache preflight:
  - `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly`
- Rust LIVE-02 gates after preflight:
  - `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly -RunGates`
- Local scanner parity (GitDoctor-style):
  - `python tools/local_gitdoctor_scan.py --root . --format markdown --fail-on none`
- Local scan snapshot (latest run in this lane):
  - `docs/GITDOCTOR-75-LOCAL-SCAN-SNAPSHOT.md`
- E2E visibility (skips if binary not built):
  - `python -m pytest -q tests/e2e -m e2e`
