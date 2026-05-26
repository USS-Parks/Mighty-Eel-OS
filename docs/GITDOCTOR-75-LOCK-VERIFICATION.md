# GitDoctor 75 Lock Verification (GD75-04)

**Date:** 2026-05-25  
**Goal:** close PRJ-004 (“missing dependency lock file”) and document offline/reproducible dependency policy for an air-gapped appliance repo.

## Lock Files Present

- **Rust (Cargo):** `Cargo.lock` (committed)
- **Python (pip-tools):** `requirements-lock.txt` (committed, hash-pinned; generated with `--generate-hashes`)
- **Node tooling (integrity MCP server):** `.integrity/mcp-server/package-lock.json` (committed; `lockfileVersion: 3`)

## Cargo Offline Cache Preflight

The Rust lock file is necessary but not sufficient for an air-gapped evidence run: the local Cargo registry cache must also contain every crate version pinned in `Cargo.lock`.

Use this sequence for release evidence hosts:

```powershell
scripts\prepare-cargo-offline-cache.ps1
scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly -RunGates
```

The first command may use the network only to populate the Cargo cache from `Cargo.lock`. It then verifies the cache with `CARGO_NET_OFFLINE=true`. The second command reruns the cache verification and then executes the LIVE-02 Rust gates with `CARGO_NET_OFFLINE=true`.

For an already air-gapped host, skip preparation and run:

```powershell
scripts\prepare-cargo-offline-cache.ps1 -VerifyOnly
```

If that command fails, the host is missing at least one locked crate version and must be prepared from a connected staging host before Rust release evidence is claimed.

## Notes

- MAI production runtime does not require Node; Node usage is limited to integrity tooling under `.integrity/`.
- `requirements-lock.txt` contains per-artifact SHA-256 hashes (`--hash=sha256:...`) for supply-chain review.
- See `docs/DEPENDENCY-LOCK-POLICY.md` for the full update policy per ecosystem.
