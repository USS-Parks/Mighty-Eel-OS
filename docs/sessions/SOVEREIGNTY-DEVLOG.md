# Sovereignty Stack DEVLOG (WSF + AOG)

Build log for `PLANNING/AOG-WSF-SOVEREIGNTY-STACK-PSPR.md` (in the islandmountain.io repo).
Format per plan §0.4: prompt id · files · verify result · commit SHA. Branch: `session/SOV-1`.

---

## Phase 0 — Foundation & shared contracts

### 0.1 — Repo hygiene + reuse map — DONE (baseline pending build)
- **Toolchain baseline:** `cargo 1.95.0` / `rustc 1.95.0`; `node v24.15.0` / `npm 11.12.1`. Disk: 432 GB free on C: (no sandbox quota — native Windows).
- **Worktree:** `session/SOV-1` at `mai-worktrees/mai-SOV-1` (from `origin/main` @ `7a19c7b`).
- **Stale clone removed:** `Documents/VS Code Lamprey Repo Clone/im-mighty-eel-mai` (verified 0 uncommitted, 0 stashes, same origin, behind HEAD → recoverable from GitHub). Kills the duplicate-repo confusion.
- **`safe-edit` determination (required by 0.1):** the skill's failure mode is the CoWork Linux-mount sandbox (`/sessions/*/mnt/`) truncating writes. This session is native Windows (win32); Write/Edit hit NTFS via `C:\` paths, not the sync layer. **Downgraded MANDATORY → RECOMMENDED for this session**, not silently skipped. Retained hygiene: surgical Edits, read-back after write, `git diff --stat` before staging, stage files individually, no `git add -A`.
- **Files:** `docs/architecture/SOVEREIGNTY-REUSE-MAP.md` (authoritative extract/reuse/new map + parked list + defects-to-fix), this DEVLOG.
- **Verify:** baseline `cargo test --workspace` = **1627 passed / 0 failed / 2 ignored** (70 test binaries), exit 0. (Note: higher than the 1,196 the RC2 docs cited — count grew across later sessions.) Reuse map lists real paths ✓.
- **Commit:** `SOV-0.1`.

### 0.3–0.6 — Shared contract schemas — DONE (spec)
Written ahead of 0.2 (they're pure spec, depend on nothing; the hard order 0.2→F1 and 0.8→crates still holds).
- `contracts/identity.md` (0.3) — `fabric-identity`; workload/session/task identity, SPIFFE id, PKI binding; MAI-claim-subject compatible.
- `contracts/trust-token.md` (0.4) — `fabric-token`; MAI `SignedClaim` + budget strand + attenuation caveats; wire-superset (old claim = root token, budget-off); attenuation narrowing invariant.
- `contracts/receipt.md` (0.5) — extends `AuditEntry` with `token_id`/`envelope_id`/`spend_cents`/`model_weights_digest`/`provider`/`workflow_id`; BLAKE3 chain + ML-DSA-87 periodic sig.
- `contracts/envelope.md` (0.6) — `fabric-envelope`; seal/label/thread three-wrap; label machine-readable un-sealed (the AOG DSPM-routing hook).
- **Verify:** specs only; serde round-trip + tamper tests land with the `fabric-contracts` crate (0.8). **Commit:** `SOV-0.3..0.6`.

### Remaining in Phase 0
- **0.2** — ml-dsa timing (RUSTSEC-2025-0144) + axum 0.7/0.8 `Handler` fix. Blocks F1. (Code change; sequenced after baseline + contracts crate so the fix has a green floor.)
- **0.7** — CI upgrade (live OpenBao service container, LocalStack, workspace tests on push, tsc/vitest, cargo-audit/deny, hadolint, SBOM).
- **0.8** — `fabric-contracts` crate (types for the four schemas) + round-trip/compat/tamper tests; tag `contracts-v1`.
