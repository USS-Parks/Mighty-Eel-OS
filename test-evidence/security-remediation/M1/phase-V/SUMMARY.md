# M1 / Phase V — AF-005 production vault truth (evidence)

Prompt: V1 (backend policy) + V8 (measured readiness). Finding: AF-005 High.

## Root cause (two holes)
1. `vault_builder::build_vault` accepted the plaintext-capable `FileDev` backend in
   production (only `Stub` was refused).
2. `server.rs` set the runtime `vault_opened` outcome to an unconditional
   `RuntimeOutcome::pass` — it never opened/verified the vault.

## Changed files
- `mai-api/src/vault_builder.rs` — `FileDevInProduction`; production file-dev is
  rejected; doc matrix corrected.
- `mai-api/src/production_guard.rs` — `PROD-VAULT-001` rejects `stub | file-dev`.
- `mai-api/src/server.rs` — `vault_opened` is measured (dev backend or missing root
  in production → Fail → fail-closed startup).
- `mai-api/tests/vault_bootstrap.rs` — `production_rejects_file_dev_backend`.

## Commands + results
- `cargo fmt --check` .................................. exit 0
- `cargo check --workspace` ........................... exit 0
- `cargo clippy -p mai-api -- -D warnings -A clippy::pedantic` exit 0
- `cargo test -p mai-api --test vault_bootstrap` ..... ok (10)
- `cargo test -p mai-api --test ship_convergence --test production_guard` ok (6 + 4)

## Negative controls
- `production_rejects_file_dev_backend` — production + file-dev (root present) →
  `FileDevInProduction`.
- `production_rejects_stub_backend` / `_allow_stub_true` / `_missing_root` /
  `_empty_root` — existing, still fail closed.
- Positive: `production_accepts_zfs_when_root_exists`; file-dev accepted in
  local-dev.

## Deferred (honest — need a live ZFS/TPM environment, the V-phase live gate)
V2 real construction (PQC/TPM/audit) + remove `ZfsVault::new` from prod boot; V3
init-before-publication (mount/dataset/key/manifest/PCR); V4 encrypted model-storage
round-trip; V5 ZFS property proof; V6 snapshot/rollback; V7 deletion / cryptographic
erasure; V9 restart/migration gate (AF-005 live closure). `vault_opened` measures
backend + root today, not the deeper encryption/init/key proofs.
