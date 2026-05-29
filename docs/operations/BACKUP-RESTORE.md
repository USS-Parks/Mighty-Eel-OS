# Backup and Restore

The MAI backup/restore contract is implemented by the
`mai-admin` Cargo binary
([`tools/mai-admin/`](../tools/mai-admin/)). This document is
the operator-facing reference; for runbook-style procedures use:

- [07-back-up-node](runbooks/07-back-up-node.md) — routine backup.
- [08-restore-node](runbooks/08-restore-node.md) — bare-metal
  restore.
- [09-recover-from-failed-upgrade](runbooks/09-recover-from-failed-upgrade.md)
  — restore as part of upgrade rollback.

## Components captured

A backup is a directory with one manifest signed by the backup
signing key plus per-component sha3-256 hashes. The components,
in the order the manifest enumerates them:

1. `build_info` — daemon version, git commit, build timestamp.
2. `config_checksums` — sha3 of every file under `/etc/mai/`.
3. `api_audit_wal` — segments + checkpoints from
   `/var/lib/mai/audit/`.
4. `compliance_audit_wal` — same for
   `/var/lib/mai/audit-compliance/`.
5. `trust_bundle_cache` — `/var/lib/mai/trust/bundles/`.
6. `trust_anchors` — `.pub` files from
   `/etc/mai/trust-anchors/`.
7. `vault_snapshot_ref` — ZFS snapshot identifier (not a copy
   of vault contents).
8. `auth_key_hashes` — `[[keys]]` entries from
   `/etc/mai/auth_keys.toml`; hashes only, never plaintext.
9. `model_registry` — installed-model manifest from the daemon.
10. `reports` — generated compliance reports from
    `/var/lib/mai/reports/`.

The vault dataset is captured by **reference**, not by export.
The reference is the ZFS snapshot name; the snapshot itself
must survive on the source pool until the restore consumes it.
That is the integrity contract for vault contents.

## Manifest signature

The manifest is signed with the backup signing key (ML-DSA-87)
which is distinct from:

- the compliance report signing key, and
- the policy bundle signing key.

Three keys, three custody chains. See
[SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) for the
custody matrix.

## Backup directory layout

```
/var/backups/mai/20260523-0200/
├── manifest.json
├── manifest.json.sig
├── api_audit_wal/
│   ├── 0000000001.wal
│   ├── 0000000001.checkpoint.sig
│   └── ...
├── compliance_audit_wal/
│   └── ...
├── trust_bundle_cache/
│   └── 2026-05-23.bundle.cbor
├── trust_anchors/
│   └── signer-a.pub
├── reports/
│   └── q1-2026.json
├── auth_key_hashes.toml
├── model_registry.json
├── vault_snapshot_ref.json
├── config_checksums.json
└── build_info.json
```

`mai-admin backup verify` re-validates every component against
the manifest before reporting `OK`.

## Retention contract

Default retention policy (`/etc/mai/backup-retention.toml`):

| Tier | Keep | Pruner behavior |
|---|---|---|
| Nightly | 14 days | drops oldest beyond window |
| Weekly | 8 weeks | promotes one nightly per week |
| Monthly | 12 months | promotes one weekly per month |

The pruner refuses to remove the most recent **verified**
backup, even if retention says to. This is the floor: if the
fleet ever has zero good backups, it is by operator action,
not by automation.

Backups are not log entries — losing one does not break the
audit chain. But once a backup window has closed without
producing a verified backup, every later in-place corruption
becomes harder to recover from. Treat backup verification
failure as a Sev-3 the same day it happens.

## Restore plan vs. apply

`mai-admin restore` is two-phase:

1. `restore plan` — read-only. Verifies manifest signature,
   per-component sha3, and replays the WAL chain on the
   **backup** side. Writes nothing to the target. Exit 0
   means the backup itself is consistent.
2. `restore apply` — writes the components into the target,
   recomputes sha3 *after* each write, replays the WAL chain
   on the **restored** side, drops `source-manifest.json` and
   `restore-report.json` witnesses next to the restored tree.

Apply refuses to write to a populated target without `--force`.
The refusal is the right default; the override is for
operator-controlled cases like restoring into a known-empty
spare pool.

## DR drill matrix

`tools/mai-admin/tests/restore_e2e.rs` implements four drills.
Operators run the same flow against an isolated host as part
of the quarterly drill cadence (see
[OPERATIONS.md](OPERATIONS.md)):

| Drill | Operator scenario | Expected outcome |
|---|---|---|
| WAL tamper in backup | A backup directory was modified after creation | `plan` exits non-zero; target stays empty |
| Missing trust bundle | Backup is missing `trust_bundle_cache/` | `plan` exits non-zero; target stays empty |
| Missing model registry | Backup is missing `model_registry.json` | `plan` exits non-zero; target stays empty |
| Signed-manifest tamper | `manifest.json` edited; `manifest.json.sig` stale | `plan` exits non-zero; target stays empty |

A passing quarterly drill records: the chosen backup id, the
operator who ran the drill, the four drill outcomes (all four
should reject), and one positive round-trip (restore a clean
backup into a clean target and confirm byte-identity on
re-backup).

## What backup does NOT cover

- Hardware state (GPU SKU, NIC config, BIOS) — operator's
  responsibility, not MAI's.
- Reverse proxy config (`/etc/nginx/`, `/etc/caddy/`, etc.) —
  out of scope.
- Host OS packages — handled by the operator's standard config
  management.
- Clients' API keys after they have left the fleet — once a key
  ships to a client, MAI no longer knows where copies live.

## Failure surface

| Symptom | Likely cause | Where to look |
|---|---|---|
| `vault_snapshot_failed` | ZFS dataset misconfigured | `vault.zfs.dataset` in profile.toml; `zfs list` |
| `signature_failed` (manifest) | Manifest signing key rotated; disk corruption | Cross-reference signer fingerprint in [SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) |
| `chain_replay_failed` | Backup captured during a tampered window | Run `audit verify` against the source; investigate before discarding |
| `target_not_empty` on apply | Target dir already has content | Move the existing tree aside; do not `--force` reflexively |

## See also

- [SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) — signing
  key custody.
- [UPGRADE-ROLLBACK.md](UPGRADE-ROLLBACK.md) — backups as the
  rollback floor.
- [INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md) — when a backup
  failure becomes an incident.
