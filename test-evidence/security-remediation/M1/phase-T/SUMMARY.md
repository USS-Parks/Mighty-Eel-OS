# M1 / Phase T — AF-001 attenuation repair (evidence)

Prompt: T1–T4 (VerificationContext + parent-authenticated, fully-monotonic
attenuation). Finding: AF-001 Critical (+ AF-006 attenuate-path leg).

## Pre-change proof (0.4 quarantine, now flipped)
`crates/fabric-token/tests/security_regression.rs` froze the vulnerable behavior:
attenuate signed a child of an unsigned/wrong-key parent (signer oracle), accepted
role/tenant widening, and attenuated a revoked parent. Phase T flips each to assert
rejection and moves them into the default suite.

## Changed files
- `crates/fabric-contracts/src/token.rs` — `Attenuation.depth` (skip-if-0).
- `crates/fabric-token/src/lib.rs` — `VerificationContext`, `verify_in_context`,
  rewritten `attenuate` (parent auth + full monotonicity + lineage bound),
  `MAX_ATTENUATION_DEPTH`, new error variants.
- `crates/fabric-token/Cargo.toml` — dep `fabric-revocation`; drop dead feature.
- `crates/wsf-api/src/lib.rs` — attenuate handler builds ctx from the anchor pubkey.
- `crates/aog-apiserver/src/seal.rs` — Sealer anchor pubkey; scoped_child_token /
  mint_child authenticate the parent.
- `crates/aog-apiserver/src/auth.rs` — `token_public_key()` accessor.
- `crates/aog-apiserver/src/lib.rs` — wire anchor into the sealer at `from_raft`.
- `crates/{wsf-broker,aog-controller/vkeys,aog-controller/scheduler,aog-node/edge}`
  + a broker live test — `depth: 0` on root-token literals.
- Tests: `fabric-token/tests/{token,security_regression}.rs`,
  `aog-apiserver/tests/seal.rs` migrated to the new signature.

## Commands + results (Linux CI container, protoc installed)
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace --all-targets` ............. exit 0
- `cargo clippy --workspace -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p fabric-token` ........................ ok (5 regression + 8 unit + 4 spend)
- `cargo test -p aog-apiserver --lib` ................. ok (3)
- `cargo test -p aog-apiserver --test seal` ........... ok (2; drives mint_child)
- `cargo test -p fabric-contracts` ................... ok
- `cargo test -p wsf-broker` ......................... ok (16)
- `cargo test -p aog-node` ........................... ok (32)
- `cargo test -p aog-controller --lib` ............... ok (50)
- `cargo test -p wsf-api` ............................ ok

## Negative controls (the flipped fixtures — each is a failing control for the fix)
- unsigned parent   → `ParentUnverified`
- wrong-key parent  → `ParentUnverified`
- role widening     → `AttenuationWidens{roles}`
- tenant swap       → `AttenuationWidens{tenant_id}`
- revoked parent    → `ParentRevoked`
Plus `attenuate_narrows_and_binds_parent` proves the legitimate narrowing path
still verifies (depth = 1, parent_id bound).

## Deferred (honest)
- `cargo test --workspace` is not run here: compiling all ~40 crates' test binaries
  exhausts the container disk (rustc-LLVM ENOSPC — infra, not a test failure). The
  tests that ran before it filled had zero failures; the full suite is CI-gated.
- T5 atomic budget lineage, T6 v1 migration, T7 live OpenBao attenuation gate
  (→ PROVEN) ride the live lane.
