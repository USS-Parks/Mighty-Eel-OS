# GitDoctor 75 Baseline (GD75-01)

**Date:** 2026-05-25  
**Repo commit:** `392359f4cda17672d841b4d1e9985b443157ada2`  
**Working tree:** clean (`git status --porcelain` empty at authoring time)

This file reconciles the external PDF hard static-analysis failures (CFG-004, TST-004, TST-005, PRJ-002, PRJ-004) against the current repo state, to prevent duplicate or speculative remediation.

See `docs/GITDOCTOR-75-EVIDENCE-PACK.md` for the consolidated reviewer entry point.

## Hard-Fail Reconciliation Table

| ID | PDF finding | Current repo status | Evidence | Next owner session |
|---|---|---|---|---|
| CFG-004 | Missing `.env.example` | **Fixed**: present at repo root. | `.env.example` | GD75-15 (evidence pack) |
| PRJ-002 | Incomplete `.gitignore` | **Appears fixed**: present at repo root with broad coverage; needs scanner parity confirmation. | `.gitignore` | GD75-15 (evidence pack) |
| PRJ-004 | Missing dependency lock file | **Fixed** for Rust/Python/Node tooling: `Cargo.lock`, `requirements-lock.txt`, and `.integrity/mcp-server/package-lock.json` exist. | `docs/DEPENDENCY-LOCK-POLICY.md`, `docs/GITDOCTOR-75-LOCK-VERIFICATION.md` | GD75-15 (evidence pack) |
| TST-005 | No integration or e2e tests | **Fixed**: e2e/integration suites exist (see below); needs a manifest + marker clarity for scanners. | `tests/e2e/`, `tests/integration/`, `tests/sdk_integration.py`, `adapters/*/tests/test_integration_*.py` | GD75-03 |
| TST-004 | Test files without assertions | **Closed** in local scanner parity: the local scan no longer reports `TST-004` after refining the check to match pytest discovery. | `docs/GITDOCTOR-75-ASSERTION-AUDIT.md` | GD75-15 (evidence pack) |

## Current Test Layout (high-level)

- `tests/`: unit + integration + e2e + integrity tooling tests
- `tests/e2e/`: e2e suite (includes `test_compliance_smoke.py`)
- `tests/integration/`: integration suite (repo-level)
- `tests/sdk_integration.py`: SDK integration runner (repo-level)
- `adapters/*/tests/`: adapter unit tests plus `test_integration_live.py` and, where present, `test_integration_mock.py`
- `mai-sdk-python/tests/`: Python SDK test suite
