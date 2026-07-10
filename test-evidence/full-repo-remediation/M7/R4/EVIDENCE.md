# R4 — Model-package distribution anchor (X4/X5 close-out, milestone M7)

**Objective.** Close the supply-chain authenticity gap (revalidation finding R4, invariant A4):
a model package must install iff it verifies against a pinned distribution trust anchor; the
appliance's own per-boot key must never authenticate a package; the manifest's declared
`security.public_key_fingerprint` must be consulted, not merely parsed.

**Source.** `X4-X5-REVALIDATION-REPORT.md` §R4 (audit run at `803e85e`); roster
`PLANNING/X4-X5-CLOSEOUT-PSPR.md` Phase R, prompt R4. Base for this change: `b0def50`.

## Pre-change proof (static)

- `PqcEngine::initialize()` mints a fresh ephemeral ML-DSA-87 keypair every boot, never
  persisted; `ZfsVault::verify_signature` delegated to `pqc.verify_package(..)` — verification
  against **that self-key** (`mai-vault/src/zfs.rs` → `pqc.rs`).
- `mai-core/src/models/verify.rs` drove both the weights and the manifest signature through
  `vault.verify_signature`, so "manifest authenticated" meant "signed by this appliance's own
  boot key," not "signed by the distribution factory."
- The manifest's `security.public_key_fingerprint` was parsed into `SecurityInfo` and never
  read again — no consult anywhere in the tree.
- No distribution/factory trust anchor was loaded anywhere in the vault build.

## Change

- **`mai-vault/src/pqc.rs`** — `PqcEngine` gains a pinned model-distribution trust anchor
  (`set_distribution_anchor`, raw 2592-byte ML-DSA-87 public key, length-validated) plus a
  fail-closed `require_distribution_anchor()` posture flag. New
  `verify_model_package(data, sig)` implements the supply-chain policy: anchor pinned →
  verify against the anchor only; required-but-absent → hard error (never a self-key
  fallback); neither (dev/legacy posture) → the existing self-key check.
  `distribution_anchor_fingerprint()` exposes `sha256:<hex>` over the raw anchor key — the
  exact format package manifests declare. The self-key `verify_package` is untouched: the
  audit chain and `first_boot`'s challenge seal legitimately verify under it.
- **`mai-vault/src/zfs.rs`** — `VaultInterface::verify_signature` now delegates to
  `verify_model_package` (the policy above), and the vault exposes
  `distribution_fingerprint()`.
- **`mai-core/src/vault.rs`** — `VaultInterface` gains a defaulted
  `distribution_fingerprint() -> Option<String>` (default `None`), so every stub/mock/dev
  vault keeps legacy behavior with zero changes.
- **`mai-core/src/models/verify.rs`** — `verify_package` consults the fingerprint: when the
  vault pins an anchor, the manifest's `security.public_key_fingerprint` must equal the
  anchor's fingerprint (case-insensitive hex compare); a mismatch is a hard refusal even when
  every signature verifies. With no anchor pinned the field stays non-normative (dev).
- **`mai-api/src/vault_builder.rs`** — the ZFS arm pins the anchor from
  `<trust.anchors_dir>/mai-model-distribution.pub` (reserved id
  `MODEL_DISTRIBUTION_ANCHOR_ID`, same on-disk convention as every other trust anchor). In
  production a missing anchor file sets the required flag: the vault boots, but every package
  verification refuses until the distribution anchor is installed — fail closed, no
  self-key fallback. Local-dev without the file keeps the self-key posture.
- **`mai-vault/Cargo.toml`** — adds `sha2` (already in the workspace tree) for the
  fingerprint hash.

**Files changed:** `mai-vault/src/{pqc.rs, zfs.rs}`, `mai-vault/Cargo.toml`,
`mai-vault/tests/distribution_anchor.rs` (new), `mai-core/src/vault.rs`,
`mai-core/src/models/verify.rs`, `mai-api/src/vault_builder.rs`, `Cargo.lock`.

Note (roster file-list deviation): the roster named `mai-core/src/models/registry.rs`; the
install boundary actually lives in `models/install.rs` + `registry.rs` (crate root) and needed
no change — the fingerprint/anchor refusals fold into `VerificationResult::verified`, which
`install_package` already enforces. `mai-core/src/vault.rs` (not on the roster list) carries
the one-trait-method seam that lets `verify.rs` see the anchor fingerprint without touching
any other vault implementation. `mai-vault/src/init.rs` needed nothing.

## New tests (`mai-vault/tests/distribution_anchor.rs` — real ML-DSA-87 keys, real vault)

- `anchor_signed_package_verifies_and_fingerprint_matches` — the roster's install gate: an
  anchor-signed v2 package verifies (`verified`, `manifest_authenticated`), and the
  fingerprint consult is recorded; fingerprint format asserted (`sha256:` + 64 hex).
- `self_key_signed_package_is_refused` — a package signed by the appliance's own engine key
  (the exact thing the old path trusted) is refused.
- `foreign_key_signed_package_is_refused` — a third-party ML-DSA key is refused.
- `fingerprint_mismatch_is_refused_even_with_valid_signatures` — anchor-signed package whose
  manifest declares a different key: signatures verify, the package is still refused, and the
  mismatch is named in the messages.
- `required_anchor_missing_fails_closed` — the production posture with no anchor installed
  refuses every package with an explicit "no model-distribution trust anchor pinned" message.

## Commands and exit codes

| Command | Result | Exit |
|---|---|---|
| `cargo fmt --check -p mai-core -p mai-vault -p mai-api` | clean | 0 |
| `cargo clippy -p mai-core -p mai-vault -p mai-api --all-targets -- -D warnings -A clippy::pedantic` | no issues | 0 |
| `cargo test -p mai-vault --test distribution_anchor` | 5 passed | 0 |
| `cargo test -p mai-core -p mai-vault -p mai-api` | 660 passed, 30 suites | 0 |
| `cargo test --workspace -j 4` | 2278 passed, 0 failed, 8 ignored (230 suites) | 0 |
| `cargo audit` | no vulnerabilities | 0 |
| `cargo deny check advisories bans licenses` | ok / ok / ok | 0 |
| `git diff HEAD \| gitleaks stdin` (change set) | no leaks found | 0 |
| `detect-secrets scan <changed .rs>` | no findings | 0 |
| `.integrity/scripts/no-slop-scan.sh full` | clean | 0 |
| `.integrity/scripts/verify-tree.sh <8 changed files>` | 8/8 passed | 0 |

(First workspace run hit `LNK1318: Unexpected PDB error; LIMIT` on the `maintenance` test
binary — an MSVC parallel-link PDB-server flake, not a compile/test failure; the bounded-
parallelism re-run is the recorded result.)

No OpenBao live gate applies: the trust boundary here is package verification against pinned
key material, proven with real ML-DSA-87 keys through the real `ZfsVault` + `PqcEngine` and
mai-core's real `verify_package` (not mocks).

## Negative controls observed

1. A self-key-signed package — the exact artifact the pre-change code accepted — is refused
   (`signature_valid = false`, `verified = false`).
2. A foreign-key-signed package is refused.
3. An anchor-signed package with a mismatched declared fingerprint is refused with the
   mismatch named, while its signatures verify — isolating the fingerprint consult.
4. Production posture with no anchor: refusal names the missing anchor; no silent self-key
   fallback exists on any path.

## Commit

`2e50ecd` — remediation(R4): pin a distribution trust anchor for model packages
(branch `session/AUDIT-FIX-2`, base `b0def50`; approved by Basho, pushed to `origin/main`).

Source comments and test names carry no roster step-codes (CANON §11, enforced by the
pre-commit no-slop PROV gate); the R4 mapping lives here, in the DEVLOG, and in git history —
the in-code vocabulary is "distribution anchor" / "supply-chain policy".
