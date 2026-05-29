# Contributor Entry Map (GitDoctor 75)

**Date:** 2026-05-25  
**Audience:** new engineers, external reviewers, and acquirer diligence teams.

This repo is intentionally multi-language (Rust core + Python adapters/SDK + small Node tooling). This map is the “first 30 minutes” path to get oriented without reading the full session logs.

## Start Here (by goal)

- **Understand the system + what not to break:** `docs/HANDOFF.md`
- **Find any document quickly:** `docs/INDEX.md`
- **Call the REST API directly:** `docs/API-REFERENCE.md`
- **Machine-readable REST contract:** `docs/api/openapi.yaml`
- **Use the Python SDK:** `docs/SDK-REFERENCE.md`
- **Understand the trust boundary + air-gap posture:** `ARCHITECTURE.md` and `docs/AIR-GAP-BRIEF.md`
- **Run production-readiness checks:** `docs/RELEASE-GATES.md` and `mai-api/src/bin/mai_ship_validate.rs`

## Codebase “front doors”

- **REST API server:** `mai-api/`
- **Core types / policy / audit contracts:** `mai-core/` and `mai-compliance/`
- **Scheduler / placement / batching:** `mai-scheduler/`
- **Python adapters:** `adapters/`
- **Python SDK:** `mai-sdk-python/`
- **Integrity tooling (Node):** `.integrity/`

## Quick Verification (local, offline)

- Rust workspace checks: `cargo check --workspace`
- Python quick tests (typical): `python -m pytest -q`

