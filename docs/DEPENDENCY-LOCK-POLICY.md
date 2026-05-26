# Dependency Lock Policy (GitDoctor / Reproducibility)

**Date:** 2026-05-25  
**Scope:** MAI Rust workspace, Python monorepo deps, and Node-based integrity tooling.

This repo targets an air-gapped appliance deployment model. Reproducibility and supply-chain review are enforced by committing lock files where dependency resolution occurs.

## Rust (Cargo)

- **Lock file:** `Cargo.lock` (committed)
- **Policy:** lock is required for reproducible builds and review of transitive dependency graphs.
- **Update:** run `cargo update` only when intentionally upgrading, then review diffs.
- **Offline evidence preflight:** before running release evidence in an air-gapped or `CARGO_NET_OFFLINE=true` environment, run `scripts\prepare-cargo-offline-cache.ps1` while network access is still allowed. The script fetches the exact locked crate set, then immediately proves `cargo fetch --locked` succeeds with `CARGO_NET_OFFLINE=true`. In an already air-gapped environment, run `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly`; any failure means the cache does not match `Cargo.lock` and the Rust evidence gates are not reproducible yet.
- **Gate command wrapper:** after a successful preflight, `scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly -RunGates` runs the LIVE-02 Rust evidence commands under `CARGO_NET_OFFLINE=true`: `cargo check --workspace`, `cargo clippy --workspace -- -D warnings -A clippy::pedantic`, `cargo test --workspace`, and `cargo deny check`.

## Python (pip / uv / pip-tools)

- **Lock file:** `requirements-lock.txt` (committed, hash-pinned)
- **Policy:** lock must include hashes (`--hash=sha256:...`) for every distribution artifact.
- **Update:** regenerate from `pyproject.toml` using the documented lock command embedded at the top of `requirements-lock.txt`, then review diffs.

## Node (tooling only)

- **Lock file:** `.integrity/mcp-server/package-lock.json` (committed)
- **Policy:** Node dependency resolution is allowed only for the integrity MCP tooling; production MAI runtime does not require Node.
- **Update:** only when intentionally changing `.integrity/mcp-server/package.json`; review all transitive changes in the lock diff.
