# GitDoctor 75 Rescan Instructions (GD75-16)

**Date:** 2026-05-25  
**Goal:** make the next external scan reproducible and ensure the scanner sees the intended evidence.

## What To Scan

- Repository root: `mai/`
- Include:
  - `docs/GITDOCTOR-75-EVIDENCE-PACK.md` (entry point)
  - `docs/api/openapi.yaml` (OpenAPI contract)
  - `tests/e2e/test_compliance_smoke.py` (e2e evidence; may skip if binary not built)
  - `Cargo.lock`, `requirements-lock.txt`, `.integrity/mcp-server/package-lock.json` (lock evidence)

## Pre-Scan Sanity (offline)

- Rust offline cache verification:
  - `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly`
- Rust LIVE-02 gates after cache verification:
  - `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly -RunGates`
- Local GitDoctor-style scan:
  - `python tools/local_gitdoctor_scan.py --root . --format markdown --fail-on none`
- Targeted acceptance tests (fast):
  - `cargo test -p mai-api --test ship_11_observability`
  - `cargo test -p mai-api --test auth_gate_a gate_a_rate_limit_returns_429`

## Notes For Reviewers

- MAI is intentionally localhost-bound and air-gapped by default. Some generic web-service "scalability" recommendations are addressed via local load-balancing and batching design evidence rather than multi-node deployments.
