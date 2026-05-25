# GitDoctor 75 Lock Verification (GD75-04)

**Date:** 2026-05-25  
**Goal:** close PRJ-004 (“missing dependency lock file”) and document offline/reproducible dependency policy for an air-gapped appliance repo.

## Lock Files Present

- **Rust (Cargo):** `Cargo.lock` (committed)
- **Python (pip-tools):** `requirements-lock.txt` (committed, hash-pinned; generated with `--generate-hashes`)
- **Node tooling (integrity MCP server):** `.integrity/mcp-server/package-lock.json` (committed; `lockfileVersion: 3`)

## Notes

- MAI production runtime does not require Node; Node usage is limited to integrity tooling under `.integrity/`.
- `requirements-lock.txt` contains per-artifact SHA-256 hashes (`--hash=sha256:...`) for supply-chain review.
- See `docs/DEPENDENCY-LOCK-POLICY.md` for the full update policy per ecosystem.

