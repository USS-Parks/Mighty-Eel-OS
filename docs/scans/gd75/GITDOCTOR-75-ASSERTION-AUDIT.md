# GitDoctor 75 Assertion Audit (GD75-02)

**Date:** 2026-05-25  
**Goal:** close or refute TST-004 (“test files without assertions”) without adding meaningless token assertions.

## What The Local Scanner Flagged (before audit refinement)

The local `tools/local_gitdoctor_scan.py` check `TST-004` was previously over-inclusive because it treated any code under `tests/` or `adapters/*/tests/` as a “test file”, including:
- `conftest.py` fixture modules
- helper modules prefixed with `_` (e.g. local test servers)
- benchmark utilities under `tests/benchmarks/`

These files are not pytest-discovered test modules and should not be judged by “has assertions”.

## Audit Rule (scanner-aligned with pytest discovery)

`TST-004` now evaluates Python files that match pytest test-module naming:
- included: `test_*.py`, `*_test.py`
- excluded: `conftest.py`, `__init__.py`, `_*.py`, non-matching utility files

## Current Result

Re-running `tools/local_gitdoctor_scan.py` on the repo produces **no TST-004 findings** as of this session, and the scanner unit tests pass (`python -m pytest tools/local_gitdoctor_tests -q`).

