# Dependency Lock Policy (GitDoctor / Reproducibility)

**Date:** 2026-05-25  
**Scope:** MAI Rust workspace, Python monorepo deps, and Node-based integrity tooling.

This repo targets an air-gapped appliance deployment model. Reproducibility and supply-chain review are enforced by committing lock files where dependency resolution occurs.

## Rust (Cargo)

- **Lock file:** `Cargo.lock` (committed)
- **Policy:** lock is required for reproducible builds and review of transitive dependency graphs.
- **Update:** run `cargo update` only when intentionally upgrading, then review diffs.

## Python (pip / uv / pip-tools)

- **Lock file:** `requirements-lock.txt` (committed, hash-pinned)
- **Policy:** lock must include hashes (`--hash=sha256:...`) for every distribution artifact.
- **Update:** regenerate from `pyproject.toml` using the documented lock command embedded at the top of `requirements-lock.txt`, then review diffs.

## Node (tooling only)

- **Lock file:** `.integrity/mcp-server/package-lock.json` (committed)
- **Policy:** Node dependency resolution is allowed only for the integrity MCP tooling; production MAI runtime does not require Node.
- **Update:** only when intentionally changing `.integrity/mcp-server/package.json`; review all transitive changes in the lock diff.

