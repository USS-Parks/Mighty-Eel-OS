# GitDoctor 75 Integration + E2E Manifest (GD75-03)

**Date:** 2026-05-25  
**Goal:** make integration/e2e coverage obvious to humans and scanners without changing MAI’s air-gapped appliance design.

## Test Suites

- **Unit tests (Python):** `tests/` and `adapters/*/tests/test_adapter.py`
- **Integration (repo-level):** `tests/integration/` (currently holds an integration-contract guard test; mark with `@pytest.mark.integration`)
- **E2E (server subprocess):** `tests/e2e/` (mark with `@pytest.mark.e2e`)
- **SDK integration runner (requires running server):** `tests/sdk_integration.py`
- **Adapter live-backend integration (optional):** `adapters/*/tests/test_integration_live.py` (mark with `@pytest.mark.live_backend`)
- **Adapter mock integration (no live backend):** `adapters/*/tests/test_integration_mock.py` (where present; mark with `@pytest.mark.integration`)

## Pytest Markers

Markers are declared in `pyproject.toml` under `[tool.pytest.ini_options]`:
- `integration`: requires a real MAI instance (or adapter mock where documented)
- `e2e`: spawns the real `mai-api` binary as a subprocess
- `live_backend`: requires a reachable backend (GPU/local runtime) and is optional by default

## How To Run (typical)

- Default fast suite: `python -m pytest -q`
- E2E only: `python -m pytest -q tests/e2e -m e2e`
- Integration only: `python -m pytest -q -m integration`
- Live backend (optional): `python -m pytest -q -m live_backend`

## Notes / Skip Semantics

- `tests/e2e/test_compliance_smoke.py` spawns the real `lamprey-mai-api` binary and **skips** with a clear message if the binary is not built. Typical build step: `cargo build --release -p mai-api`.
- `tests/sdk_integration.py` expects a MAI server already running (defaults to `http://localhost:8420/v1`) and uses `MAI_TEST_API_KEY` for authenticated calls; it is not expected to pass in a fresh checkout without a running appliance/server.
