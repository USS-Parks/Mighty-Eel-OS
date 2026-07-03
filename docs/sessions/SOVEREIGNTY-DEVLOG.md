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
- **Commit:** `SOV-0.1` (cdfb05f).

### 0.3–0.6 — Shared contract schemas — DONE (spec)
Written ahead of 0.2 (they're pure spec, depend on nothing; the hard order 0.2→F1 and 0.8→crates still holds).
- `contracts/identity.md` (0.3) — `fabric-identity`; workload/session/task identity, SPIFFE id, PKI binding; MAI-claim-subject compatible.
- `contracts/trust-token.md` (0.4) — `fabric-token`; MAI `SignedClaim` + budget strand + attenuation caveats; wire-superset (old claim = root token, budget-off); attenuation narrowing invariant.
- `contracts/receipt.md` (0.5) — extends `AuditEntry` with `token_id`/`envelope_id`/`spend_cents`/`model_weights_digest`/`provider`/`workflow_id`; BLAKE3 chain + ML-DSA-87 periodic sig.
- `contracts/envelope.md` (0.6) — `fabric-envelope`; seal/label/thread three-wrap; label machine-readable un-sealed (the AOG DSPM-routing hook).
- **Verify:** specs only; serde round-trip + tamper tests land with the `fabric-contracts` crate (0.8). **Commit:** `SOV-0.3..0.6` (1e9f035).

### 0.8 — `fabric-contracts` crate — DONE
Pure-types crate at `crates/fabric-contracts` (new `crates/` grouping for the fabric layer; `mai-*` stay flat at root). Modules: `common` (Signature + Route/Classification/ComplianceScope/RevocationStatus/RoutingDecision), `identity`, `token` (TrustToken/Budget/Attenuation/Caveat), `receipt` (Receipt/Correlation/PeriodicSignature), `envelope` (Seal/Label/Thread). serde-only, no crypto. `token_id` aliases `claim_id`; `budget`/`attenuation` are `#[serde(default)]` so MAI claims deserialize as root tokens.
- **Files:** `crates/fabric-contracts/{Cargo.toml, src/*.rs, tests/contracts.rs}`; workspace `Cargo.toml` member add.
- **Verify:** `cargo test -p fabric-contracts` = **5 passed** (round-trip ×4 incl. the real TRUST-MANIFOLD §4.1 MAI claim; label-standalone ×1); `cargo clippy -p fabric-contracts -- -D warnings -A clippy::pedantic` clean (fixed one `derivable_impls`); `cargo check --workspace` green (integrates, 0 regressions). Compiler + pre-commit hook stand in for the CoWork-era subagent file-verify.
- **Commit:** `SOV-0.8`. **Tag:** `contracts-v1`.

### 0.2a — `fabric-crypto` signer abstraction — DONE
The confirmed decision (trait + pure-Rust default + Transit custody seam), in code. `crates/fabric-crypto`: `Signer`/`Verifier` traits over raw bytes; `RustCryptoMlDsa87` provider (pure-Rust ML-DSA-87, a mirror of mai-vault's proven `pqc-dev` `dsa_backend` — **no new crypto library introduced**) as the offline/air-gap default; `TransitSigner` provider as the OpenBao-Transit custody seam that **fails closed** until Phase W (OSS Transit has no GA ML-DSA; only Vault Enterprise does, experimentally). Byte-oriented, no fabric-contracts coupling — `fabric-proof` will map sigs to the wire `Signature`.
- **Verify:** `cargo test -p fabric-crypto` = **4 passed** (sign/verify round-trip; tamper + wrong-key + wrong-size fail-closed; `from_keypair` reconstruct; Transit fails-closed); `cargo clippy … -D warnings -A pedantic` clean; compiled first try on the proven ml-dsa 0.0.4 API.
- **Commit:** `SOV-0.2a`.

### Remaining in Phase 0 (0.2 sub-steps + 0.7)
- **0.2b** — DONE. Dropped the `pqc-prod` feature + archived `pqcrypto-mlkem/mldsa/traits` deps from mai-vault; pure-Rust `pqc-dev` is now the sole backend (cfg gates simplified `pqc-dev`/`not(pqc-prod)` → `pqc-dev`, compile-guard + module docs updated). Confirmed nothing selected `pqc-prod` (grep: only its own feature def + docs + one historical test-run line — no ship profile / CI / production_guard). User decision (2026-07-03): drop FIPS-liboqs for now, re-add via maintained `oqs` behind a new feature if ITAR needs it. **Verify:** `cargo test -p mai-vault` = **63 passed**; `cargo check --workspace` green; `cargo audit` unmaintained warnings **6→2** (all four `pqcrypto-*` gone). **Commit:** `SOV-0.2b`.
- **0.2c** — DONE. `cargo update` bumped **anyhow 1.0.102→1.0.103** (RUSTSEC-2026-0190 unsoundness) + **quinn-proto 0.11.14→0.11.15** (RUSTSEC-2026-0185, HIGH 7.5). **pyo3 0.24 ×2** (RUSTSEC-2026-0176/0177): grep proved the vulnerable APIs (`new_closure`/`PyList`/`PyTuple`/`.nth`) have **zero** uses in the workspace + pyo3 is mai-adapters-only (parked) → documented waiver in `.cargo/audit.toml` + `deny.toml` + DEFERRALS §1.4, not a risky 5-minor major bump. Retired the now-stale `paste` waiver (RUSTSEC-2024-0436, gone with pqcrypto in 0.2b; DEFERRALS §1.3 → RESOLVED). **Verify:** `cargo audit` **exit 0, 0 vulnerabilities** (1 accepted non-failing warning: proc-macro-error2 unmaintained, transitive via `validator`→mai-api, pre-existing). **Commit:** `SOV-0.2c`.
  - *Correction:* the plan's `RUSTSEC-2025-0144` (ml-dsa timing) was already **waived in-repo** (air-gap-mitigated), not absent — `fabric-crypto` now de-risks its eventual fix to a single provider swap (DEFERRALS §1.1 note).
- **0.2d** — tonic 0.12→0.13 to align on axum 0.8, retiring the dual-`Handler` `post_service` workaround.

### Remaining in Phase 0
- **0.2** — DECISION (2026-07-03; plan premise corrected). RUSTSEC-2025-0144 (ml-dsa timing) does **not** fire in `cargo audit`. Real advisory state: pyo3 0.24 ×2 (→≥0.29), quinn-proto 0.11.14 HIGH 7.5 (→≥0.11.15), anyhow unsoundness, and the load-bearing one — the **pqcrypto PQClean family is unmaintained/archived** (mai-vault signs via `mldsa87::detached_sign`; mai-compliance via RustCrypto `ml-dsa` 0.0.4). Verified: ML-DSA Transit signing exists only in **Vault Enterprise 1.19 (experimental)**; **open-source OpenBao Transit does not** list ML-DSA. Depending on Vault Enterprise contradicts the sovereignty thesis, and air-gap/Ring-3 needs local signing regardless. **Chosen path:** (a) abstract signing behind a `Signer`/`Verifier` trait in `fabric-proof`; default provider = a maintained pure-Rust ML-DSA/ML-KEM impl (verify `fips204`/`fips203` vs RustCrypto maturity), dropping the archived pqcrypto bindings and unifying the two paths; OpenBao Transit becomes a **pluggable custody provider behind the same trait**, lit up when OSS PQ Transit is GA. (b) bump pyo3/quinn-proto/anyhow. **Risk to verify:** cross-impl signature/key interop (PQClean vs FIPS-204-final encodings) — else regen test fixtures. Blocks F1.
- **axum** — resolve 0.7/0.8 dual-`Handler` by aligning tonic→0.13 (uses axum 0.8) so gateway handlers use plain `post(handler)`. (Folded into 0.2.)
- **0.7** — CI upgrade (live OpenBao service container, LocalStack, workspace tests on push, tsc/vitest, cargo-audit/deny, hadolint, SBOM).
