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

## Interlude — Agentic Security Map introduced as canonical AOG spec (2026-07-03)
Basho introduced **"The Agentic Orchestration & Security Map"** (July 2026 — his own blog post + infographic) as the canonical threat model + control-plane doctrine for AOG/WSF. The marketing map is now the engineering design contract. Added `docs/architecture/AGENTIC-SECURITY-MAP.md`: the threat→control→where-it-lives→status table (9 OWASP-LLM-aligned threats) + 6 net-new enrichments (E-A orchestration-pattern governance, E-B memory/RAG provenance+quarantine, E-C session integrity/signed checkpoints, E-D tool supply-chain, E-E sandboxed execution, E-F OWASP LLM Top 10 evidence). The stack VALIDATES on ~8 controls (identity/least-priv/policy/HITL/secrets/budgets/egress/audit) and ENRICHES with the six. PSPR updated with a Threat Model appendix. Build paused mid-Phase-0 for this integration; resumes at 0.2d.

### Remaining in Phase 0 (0.2 sub-steps + 0.7)
- **0.2b** — DONE. Dropped the `pqc-prod` feature + archived `pqcrypto-mlkem/mldsa/traits` deps from mai-vault; pure-Rust `pqc-dev` is now the sole backend (cfg gates simplified `pqc-dev`/`not(pqc-prod)` → `pqc-dev`, compile-guard + module docs updated). Confirmed nothing selected `pqc-prod` (grep: only its own feature def + docs + one historical test-run line — no ship profile / CI / production_guard). User decision (2026-07-03): drop FIPS-liboqs for now, re-add via maintained `oqs` behind a new feature if ITAR needs it. **Verify:** `cargo test -p mai-vault` = **63 passed**; `cargo check --workspace` green; `cargo audit` unmaintained warnings **6→2** (all four `pqcrypto-*` gone). **Commit:** `SOV-0.2b`.
- **0.2c** — DONE. `cargo update` bumped **anyhow 1.0.102→1.0.103** (RUSTSEC-2026-0190 unsoundness) + **quinn-proto 0.11.14→0.11.15** (RUSTSEC-2026-0185, HIGH 7.5). **pyo3 0.24 ×2** (RUSTSEC-2026-0176/0177): grep proved the vulnerable APIs (`new_closure`/`PyList`/`PyTuple`/`.nth`) have **zero** uses in the workspace + pyo3 is mai-adapters-only (parked) → documented waiver in `.cargo/audit.toml` + `deny.toml` + DEFERRALS §1.4, not a risky 5-minor major bump. Retired the now-stale `paste` waiver (RUSTSEC-2024-0436, gone with pqcrypto in 0.2b; DEFERRALS §1.3 → RESOLVED). **Verify:** `cargo audit` **exit 0, 0 vulnerabilities** (1 accepted non-failing warning: proc-macro-error2 unmaintained, transitive via `validator`→mai-api, pre-existing). **Commit:** `SOV-0.2c`.
  - *Correction:* the plan's `RUSTSEC-2025-0144` (ml-dsa timing) was already **waived in-repo** (air-gap-mitigated), not absent — `fabric-crypto` now de-risks its eventual fix to a single provider swap (DEFERRALS §1.1 note).
- **0.2d** — DONE (resolved by **isolation**, not a risky bump). The axum 0.7/0.8 dual-`Handler` conflict (KNOWN-ISSUES #7) is a **mai-api-legacy artifact**: mai-api pulls axum 0.7 via tonic 0.12 AND uses axum 0.8 directly. New WSF/AOG service crates avoid it **by construction** — they pin **tonic 0.14 + axum 0.8** (tonic ≥0.13 aligned to axum 0.8), so no single crate mixes `Handler` versions and gateway handlers use plain `post(handler)`. Fabric crates are already tonic-free. A full mai-api tonic 0.12→0.14 migration (26K LOC of gRPC, off the WSF/AOG critical path, workaround works + 1627 tests green) is deferred to its own task — same judgment as the pyo3 waiver. **Workspace convention (new):** WSF/AOG service crates declare `tonic`/`prost` **0.14** + `axum` **0.8** directly (not the legacy workspace 0.12/0.13); pinned + empirically verified at **W6** (first gRPC service crate). **Commit:** `SOV-0.2d`.
- **0.7** — DONE (scoped to what's runnable now). Added an **`advisories`** job to `ci.yml`: `cargo audit` (reads `.cargo/audit.toml` ignores) + `cargo deny check advisories bans licenses` (reads `deny.toml`) — a hard gate on every push/PR that locks the 0.2c advisory posture in CI. (`rust-check` already runs `cargo check`/`clippy -D warnings`/`fmt --check`/`test --workspace` — covers both new fabric crates — plus hadolint.) **Deferred honestly** (they test surfaces that don't exist yet): live-OpenBao + LocalStack → **Phase W** (first live-trust / cred-broker tests, the no-mock-only gate); tsc/vitest → **Phase C** (console); SBOM (syft) + image signing → **D3**. **Verify:** yaml structure valid; runs on next push (not pushed — Basho's call). **Commit:** `SOV-0.7`.

## ✅ Phase 0 COMPLETE (2026-07-03)
All items done: 0.1, 0.2a–d, 0.3–0.6, 0.7, 0.8 + tag `contracts-v1`. **Nine commits** on `session/SOV-1` (`7a19c7b..` HEAD). Deliverables: reuse map + Agentic Security Map (canonical spec); four frozen wire contracts (`fabric-contracts`, 5 tests); signer abstraction (`fabric-crypto`, 4 tests — pure-Rust ML-DSA default + OpenBao Transit seam); archived pqcrypto removed (pure-Rust sole backend); every advisory cleared (`cargo audit` exit 0) + CI advisories gate. Baseline held green (full `cargo test --workspace` 1627+). **Next: F1 — extract `fabric-proof` (audit chain + canonical bundle verify + subject-hash from mai-compliance) on top of `fabric-crypto`; mai-compliance re-exports so its 326+ tests stay green.**

---

## Phase F — Fabric crates

### F1 — `fabric-proof` extracted — DONE
`crates/fabric-proof`: the shared audit-proof primitives on top of `fabric-crypto`. Modules: `canonical` (`write_canonical` + `canonical_bytes`/`canonical_hash` + `combined_hash` — **byte-identical** to mai-compliance BF-3), `subject_hash` (HMAC-SHA256 string-core), `bundle` (`BundleVerifier` trait + `MlDsaBundleVerifier` over fabric-crypto's ML-DSA-87 verifier + anchor registry), `chain` (BLAKE3 hash-chain primitive for WSF's receipt ledger — `GENESIS_HASH`/`chain_link`/`ChainLink`/`verify_chain`). Dep-light: blake3/hmac/sha2/serde/serde_json/hex/fabric-crypto.
- **Extraction (real, in-place):** mai-compliance now **delegates** to fabric-proof — `subject_hash::hmac_subject` wraps `fabric_proof::hmac_subject` (its `SubjectId`/`SubjectHash` newtypes + `SubjectHashError` preserved), and `bundle::write_canonical` routes through `fabric_proof::write_canonical` (so `payload_hash` + the canonical encoding are single-sourced). Deeper audit-chain + `MlDsaBundleVerifier` migration in mai-compliance is **staged** (deeply integrated across ~23 files; fabric-proof is the single source for new WSF/AOG code and is proven wire-compatible).
- **Verify:** `cargo test -p fabric-proof` = **5 passed** (canonical byte-parity with mai-compliance; subject-hash spec; sign-with-fabric-crypto → verify-with-fabric-proof round-trip + tamper + unknown-anchor; chain link + break-at-index detection); `cargo test -p mai-compliance` = **331 lib + 10 integration passed, 0 failed** (regression guard GREEN through the delegation); clippy clean; `cargo check --workspace` green. **Commit:** `SOV-F1`.

### F3 — `fabric-token` — DONE
`crates/fabric-token`: the WSF primitive over `fabric_contracts::TrustToken`, signed via `fabric-crypto`, hashed via `fabric-proof`. Four ops: `issue` (sign the canonical payload with the `signature` field excluded), `verify` (signature + revocation; expiry via `is_expired` with chrono), `attenuate` (mint a child that narrows the parent on **every** axis — routes/models subset, classification ceiling, budget ≤ parent-remaining, expiry ≤ parent — fails closed on any widening, binds `attenuation.parent_id`), `try_spend` (atomic budget metering; no-op when the budget strand is absent).
- **Verify:** `cargo test -p fabric-token` = **8 passed** (issue→verify round-trip; tamper rejected; revoked rejected; attenuate narrows + binds parent; attenuate rejects widening on routes/classification/budget; budget meters + stops at cap without partial commit; no-budget no-op; expiry before/after); clippy clean. **Commit:** `SOV-F3`. (Did F1 then F3 — both build on the fabric-proof/crypto/contracts done in Phase 0/F1; F2 `fabric-identity` next.)

### F2 — `fabric-identity` — DONE
`crates/fabric-identity`: mint/verify signed `fabric_contracts::Identity` assertions (via fabric-crypto), derive short-lived **Session/Task** child identities bound to a parent (the loop → session → task chain, inheriting tenant/subject/service), and pseudonymize subjects (via fabric-proof). PKI-leaf binding (`pki_cert_fingerprint`) is the Phase-W OpenBao-PKI seam — carried through unchanged here.
- **Verify:** `cargo test -p fabric-identity` = **5 passed** (mint→verify; tamper rejected; session-child binds parent + inherits + verifies; non-Session/Task child kind rejected; pseudonymize deterministic + tenant-key-length guard); clippy clean. **Commit:** `SOV-F2`.

### 0.2 decision record (historical)
- **0.2 crypto** — DECISION (2026-07-03): the plan's RUSTSEC-2025-0144 premise was corrected — the real issue was the archived pqcrypto PQClean family (+ pyo3/quinn-proto/anyhow). Chosen path: `Signer`/`Verifier` abstraction + pure-Rust ML-DSA default + OpenBao Transit as a pluggable custody seam (OSS Transit lacks GA ML-DSA; only Vault Enterprise 1.19, experimental; air-gap needs local signing). Shipped by **0.2a** (fabric-crypto) + **0.2b** (drop pqc-prod) + **0.2c** (advisory cleanup).
- **axum 0.7/0.8 dual-`Handler`** — resolved by **0.2d** (isolation: new crates on tonic 0.14/axum 0.8; mai-api migration deferred).
