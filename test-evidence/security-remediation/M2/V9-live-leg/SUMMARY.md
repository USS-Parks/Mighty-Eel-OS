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

**2. Mechanism prerequisites (V5/V6) not yet wired.** V9 presupposes the V5 dataset property
proof and the V6 real snapshot/rollback operations. In `mai-vault/src/zfs.rs` the snapshot
ops are still metadata-only placeholders — `create_snapshot` / `rollback_snapshot` /
`delete_snapshot` update in-memory metadata with comments *"In production: run `zfs snapshot
im-vault/models@{name}`"* (zfs.rs ~460, ~483, ~503) and never execute bounded `zfs` argv.
Even on a real ZFS+TPM host, V9 cannot go green until V5/V6 land.

## Exact prerequisites for the V9 live run

1. Host with the OpenZFS kernel module + `zfsutils`, a disposable pool/dataset for the
   fixture, and TPM 2.0 (`/dev/tpmrm0`, or `swtpm` for a software rig).
2. V5 implemented: readiness queries the actual dataset (encryption, keystatus, mountpoint,
   readonly, quota) and fails against a plain directory masquerading as ZFS.
3. V6 implemented: bounded-argv `zfs snapshot/rollback/destroy` with validated identifiers
   and audit receipts, integration-tested on a disposable dataset.
4. Then the V9 sequence itself, each step with a failing negative control (V8 rule).

## Verdict

**V9 stays open — correctly.** The plan's own rules forbid a pass here: V8 requires every
reported pass to have a failing negative control on the real backend, and M2's outcome is
*"honestly gated"*. The offline leg is green and recorded; the live leg awaits the designated
ZFS+TPM host **and** completion of the V5/V6 mechanisms. Marking V9 green from this
environment would recreate the exact false-readiness finding (AF-005) the gate exists to
close.

## Files

- `v9-host-feasibility.txt` — timestamped host probe (kernel modules, zfs, TPM, egress)
- `mai-vault-offline.log` — offline suite output; **local artifact only** (`*.log`
  gitignored). Verbatim result line: `test result: ok. 63 passed; 0 failed; 0 ignored;
  0 measured; 0 filtered out; finished in 0.30s` — regenerate with `cargo test -p mai-vault`
