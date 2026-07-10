# R5 — Vault seal readiness honesty (X4/X5 close-out, milestone M7)

**Objective.** Close the seal-readiness theater (revalidation finding R5, invariant A5): wire
master-key sealing onto the production boot path and replace the config-flag certification of
the seal with a runtime probe that fails closed on a no-seal backend.

**Source.** `X4-X5-REVALIDATION-REPORT.md` §R5 (audit run at `803e85e`); roster
`PLANNING/X4-X5-CLOSEOUT-PSPR.md` Phase R, prompt R5. Base for this change: `1cf766b`.

**Scope note (unchanged owner/hardware lane).** Real-TPM seal verification (hardware
`/dev/tpmrm0` binding) stays deferred per the roster. This prompt makes the boot path seal
through the configured seal provider — and refuse when it cannot — and makes readiness
measure that seal at runtime. When the hardware lane lands, the same probe binds to the real
device.

## Pre-change proof (static)

- `PROD-VAULT-004` passed on a **config flag**: `if p.vault.require_sealed_master_key { pass }`
  (`mai-api/src/production_guard.rs`) — no runtime relationship to any seal.
- `PROD-VAULT-100`'s runtime probe was a store→load round-trip (`probe_vault`) that never
  asserts sealing.
- `first_boot()` — the only sealing path — was called only from its own test; the production
  boot path (`build_vault`) never sealed anything.
- The key-store KEK (root of restart recovery, wraps every persisted model key) was written as
  a **plaintext file**: `std::fs::write(<key_store>/kek.bin, k)` in `mai-vault/src/pqc.rs`.

Effect: readiness certified "sealed master key" on every production boot while no sealing
existed and the master KEK sat in plaintext.

## Change

- **`mai-vault/src/pqc.rs`** — `PqcEngine` gains an optional TPM seal provider
  (`set_seal_provider`). With one wired, the KEK exists on disk only as the sealed envelope
  `kek.sealed` (`KEK_KEY_ID = "vault-master-kek"`): fresh KEKs are sealed at birth, a legacy
  plaintext `kek.bin` is migrated into the envelope (same value — existing wrapped model keys
  stay decryptable) and the plaintext deleted, and any seal/unseal failure (TPM unavailable,
  PCR drift, tampered envelope) is a hard error — no plaintext fallback. `initialize()` now
  materializes the KEK at boot instead of lazily at first model install, so sealing happens on
  the boot path and is immediately probeable. New `probe_sealed_master_key()` is the runtime
  assertion: provider available + envelope present + unseals under the **current** PCR state to
  a well-formed key + no plaintext residue.
- **`mai-vault/src/tpm.rs`** — `unseal_key` is now stateless (the per-process bookkeeping-map
  lookup is gone): a real TPM 2.0 unseals any blob whose PCR policy matches, with no memory of
  what it sealed — which is exactly what lets a sealed KEK written by a prior boot unseal
  after a restart. The AEAD tag under the PCR-derived key remains the binding.
- **`mai-vault/src/init.rs`** — `first_boot` wires the TPM as the engine's seal provider
  before `initialize()`, so its boot sequence seals the engine's master key material (not just
  a challenge blob).
- **`mai-api/src/vault_builder.rs`** — the ZFS arm wires
  `pqc.set_seal_provider(TpmManager::new(cfg.tpm))` before `initialize()` — sealing is on the
  production boot path, fail-closed. New `probe_master_key_seal(profile)` maps the vault-layer
  probe onto a `RuntimeOutcome`; stub / file-dev backends fail closed ("no seal path").
- **`mai-api/src/production_guard.rs`** — new deferred-Critical runtime check
  **`PROD-VAULT-101`** ("master KEK is sealed and the envelope unseals under the current PCR
  state — runtime-proven, not config-asserted") wired through `RuntimeChecks.master_key_sealed`
  and `apply_runtime`. `PROD-VAULT-004` (the flag) remains as config hygiene but no longer
  certifies the seal — the measurement does.
- **`mai-api/src/server.rs`** — `MaiServer::run()` probes the seal right after `build_vault`
  and threads the outcome through `apply_ship_profile`; a missing probe fails closed, matching
  the V8 vault-probe pattern.
- **`mai-api/src/bin/mai_ship_validate.rs`** — the ship validator (systemd `ExecStartPre`)
  runs the same probe, so a stock install enforces the seal before any socket binds.

**Files changed:** `mai-vault/src/{pqc.rs, tpm.rs, init.rs}`,
`mai-api/src/{vault_builder.rs, production_guard.rs, server.rs}`,
`mai-api/src/bin/mai_ship_validate.rs`, `mai-api/tests/{ship_convergence.rs,
ship_07b_endpoints.rs}` (fixture field).

## New tests

- `pqc::tests::sealed_kek_created_at_boot_and_survives_restart` — boot seals the KEK on the
  boot path (no plaintext ever written); a fresh boot over the same store unseals it and
  decrypts pre-restart weights.
- `pqc::tests::legacy_plaintext_kek_migrates_into_sealed_envelope` — migration seals the
  same KEK value, removes the plaintext, and old wrapped model keys stay decryptable.
- `pqc::tests::probe_asserts_the_measured_seal_state` — probe refuses a store with no
  envelope; passes a sealed store; refuses plaintext residue; refuses after PCR drift
  (firmware-change simulation).
- `pqc::tests::no_seal_backend_fails_closed` — the R5 gate: with a no-seal backend both the
  boot path (`initialize()` errors, no plaintext fallback) and the probe refuse.

Source comments and test names deliberately carry no roster step-codes (CANON §11, enforced
by the pre-commit no-slop PROV gate); the R5/R8 mapping lives here, in the DEVLOG, and in the
commit history.
- `production_guard::tests::unsealed_master_key_blocks_ship_ready` — a failing seal probe
  flips `PROD-VAULT-101` to Fail and blocks readiness even while the `PROD-VAULT-004` config
  flag passes — the flag no longer certifies the seal.
- Existing guard flip/partial/skip tests extended to cover `PROD-VAULT-101`.

## Commands and exit codes

| Command | Result | Exit |
|---|---|---|
| `cargo fmt --check -p aog-gateway -p mai-vault -p mai-api` | clean | 0 |
| `cargo clippy -p mai-api -p mai-vault --all-targets -- -D warnings -A clippy::pedantic` | no issues | 0 |
| `cargo test -p mai-vault` | 90 passed, 3 suites | 0 |
| `cargo test -p mai-vault -p mai-api` | 453 passed, 26 suites | 0 |
| `cargo test --workspace` | 2273 passed, 0 failed, 8 ignored (229 suites) | 0 |
| `cargo audit` | no vulnerabilities | 0 |
| `cargo deny check advisories bans licenses` | ok / ok / ok | 0 |
| `git diff HEAD \| gitleaks stdin` (change set) | no leaks found | 0 |
| `detect-secrets scan <changed .rs>` | `results: {}` | 0 |
| `.integrity/scripts/no-slop-scan.sh all` | clean | 0 |
| `.integrity/scripts/verify-tree.sh <13 changed files>` | 13/13 passed | 0 |

No live OpenBao gate applies to R5 (the seal path is vault-local); the trust-boundary proof is
the real-key-store tests above (real files, real seal provider, real PCR-drift refusal — not
mocks).

## Negative controls observed

1. `r5_no_seal_backend_fails_closed`: on a backend that cannot seal, `initialize()` returns
   `TpmUnavailable` (boot refuses) and no plaintext KEK is written — and the probe refuses.
2. `unsealed_master_key_blocks_ship_ready`: readiness reports `PROD-VAULT-101 = Fail` and
   `is_ship_ready() = false` on an unsealed master key, while the old config flag alone would
   have passed — the exact theater the finding named, now closed.
3. `r5_probe_asserts_the_measured_seal_state`: after PCR drift the probe refuses — the check
   tracks the runtime truth, not configuration.

## Commit

`b0def50` — remediation(R5): seal the vault master KEK at boot and runtime-probe it
(branch `session/AUDIT-FIX-2`, base `72d89e7`; approved by Basho, pushed to `origin/main`).
