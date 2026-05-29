# CFG-CLEAN — Configuration & Packaging Hygiene

This note captures the "CFG-CLEAN" follow-up items referenced by the SCAN-1
internal scan report.

## What is enforced in CI

- **Lock-file parity:** `scripts/verify-lock-parity.sh` runs in CI to ensure
  `Cargo.lock` and `requirements-lock.txt` stay consistent with their
  manifests.
- **Dockerfile linting:** hadolint runs in CI using `.hadolint.yaml`.

## Repo-root hygiene policy

The repo root must stay free of transient tooling artifacts. The following
known spurious artifacts are ignored via `.gitignore`:

- `py_tmp_dir/`, `pytest_temp/`, `.pytest-tmp/`
- `pytest-cache-files-*`
- Two historically accidental root files: `12`, `et HEAD`

These are build/test byproducts and must not be committed or shipped.

