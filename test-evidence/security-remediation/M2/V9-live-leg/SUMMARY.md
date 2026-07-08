# M2 — V9 vault restart/migration gate: status after the 2026-07-07 live run

**Repo state at run:** `4af8b8b0daad47968a7f5c3c66e298c6aa53cc8a` on `claude/live-gates-docker-zfs-qoe2bc`
**Objective:** run the V9 live leg — install encrypted fixture → restart → verify/decrypt →
snapshot → migrate legacy plaintext fixture → failure recovery — on a ZFS+TPM host
(PSPR: *Gate: AF-005 closed and production claims updated*).

## Offline leg — GREEN

`cargo test -p mai-vault` → **exit 0, 63 passed / 0 failed** (see `mai-vault-offline.log`),
including the negative controls the plan demands:

- `pqc::test_dsa_tamper_detection` — tampered package fails verification
- `zfs::test_load_missing_model_fails` — absent model is an error, not a silent pass
- `zfs::test_store_duplicate_fails` — duplicate store rejected
- `zfs::test_verify_signature_requires_wired_pqc_engine` — unwired PQC engine fails closed
- `pqc::test_per_model_key_isolation` — wrong model key cannot decrypt
- `zfs::test_snapshot_lifecycle`, `zfs::test_integrity_check`, `zfs::test_initialize_creates_vault`

## Live leg — NOT RUN (blocked twice over; recorded honestly per plan §0.5 / V8)

**1. Host infeasibility (this runner).** Probe in `v9-host-feasibility.txt`:
no kernel module tree (`/lib/modules` absent → no `zfs.ko` can ever load), `zfs` absent from
`/proc/filesystems`, no `zfs`/`zpool` userspace, no `/dev/tpm*`, no TPM char-device major in
`/proc/devices`, and the egress policy blocks apt mirrors (deb.debian.org → 403), so neither
`zfsutils-linux` nor `swtpm` can even be installed. This sandbox cannot be the ZFS+TPM host.

**2. Mechanism prerequisites (V5/V6) — CLOSED in this change (2026-07-07).** V9 presupposes
the V5 dataset property proof and the V6 real snapshot/rollback operations; both are now
wired, replacing the metadata-only placeholders that were at zfs.rs ~460/483/503:

- `mai-vault/src/zfs_ops.rs` — bounded ZFS execution: direct argv (never a shell), strict
  dataset/snapshot identifier validation (charset, no leading `-`, no `..`, length caps),
  hard timeout, typed parsing. V5: `dataset_properties`/`verify_dataset` require actual
  encryption, keystatus=available, type=filesystem, mounted, pinned mountpoint, readonly=off,
  compression — a missing dataset or missing `zfs` binary is a hard error. V6: `snapshot`/
  `rollback` (un-forced — no `-r`, nothing deleted implicitly)/`destroy_snapshot`
  (snapshot-only by construction: the argv target always carries `@`)/`list_snapshots`
  (real creation times + referenced bytes).
- `mai-vault/src/zfs.rs` — `ZfsVault::with_zfs(ops)` enables the real path: `initialize()`
  runs the live property proof (V5) before touching anything; create/rollback/delete/list
  execute real `zfs` and land `SnapshotCreate`/`SnapshotRollback`/`SnapshotDelete` receipts
  on the hash-chained audit log, fail-closed (a failed receipt fails the operation);
  `storage_info` reports actual used/available/compressratio. Without `with_zfs` the vault
  keeps its previous dev/test behavior — no observable change for existing consumers.
- Offline proof: `cargo test -p mai-vault` → **74/74** (up from 63), including argv-exactness,
  injection-shape rejection, every V5 negative control (encryption off, key unavailable,
  unmounted, wrong mountpoint, readonly, compression off, non-filesystem), and the
  fail-closed-without-a-real-dataset control. `cargo clippy` with CI flags: clean;
  `cargo check --workspace`: 0 errors.
- Live leg on a ZFS host: `MAI_ZFS_TEST_DATASET=<disposable-dataset> cargo test -p mai-vault
  --test live_zfs` (`mai-vault/tests/live_zfs.rs`, env-gated, self-cleaning; provisioning
  recipe in its header) — property proof, masquerade negative control, store→snapshot→
  damage→rollback→destroy with bytes verified, signed receipts + chain verification.

## Exact prerequisites for the V9 live run (remaining)

1. Host with the OpenZFS kernel module + `zfsutils`, a disposable pool/dataset for the
   fixture, and TPM 2.0 (`/dev/tpmrm0`, or `swtpm` for a software rig).
2. ~~V5 implemented~~ / ~~V6 implemented~~ — **done** (this change); run their live leg on
   the host via `live_zfs` above.
3. V4 (encrypted model storage wiring) for the encrypted-fixture install/decrypt legs of the
   V9 sequence, then the V9 sequence itself — install encrypted fixture → restart →
   verify/decrypt → snapshot → migrate legacy plaintext fixture → failure recovery — each
   step with a failing negative control (V8 rule).

## Verdict

**V9 stays open — correctly — but its mechanism gap is now closed.** The plan's own rules
forbid a pass here: V8 requires every reported pass to have a failing negative control on the
real backend, and M2's outcome is *"honestly gated"*. The offline leg is green and recorded
(74/74 after V5/V6 landed); the live leg awaits the designated ZFS+TPM host, where `live_zfs`
closes V5/V6 live and the remaining V9 sequence (with V4) closes AF-005. Marking V9 green
from this environment would recreate the exact false-readiness finding the gate exists to
close.

## Files

- `v9-host-feasibility.txt` — timestamped host probe (kernel modules, zfs, TPM, egress)
- `mai-vault-offline.log` — offline suite output; **local artifact only** (`*.log`
  gitignored). Verbatim result line: `test result: ok. 63 passed; 0 failed; 0 ignored;
  0 measured; 0 filtered out; finished in 0.30s` — regenerate with `cargo test -p mai-vault`
