# Runbook 12 — Audit WAL Tamper Detected

> **Status against RC1 freeze (`dceaabc`):** Describes the
> production operator surface. The `mai-admin audit verify`
> command cited below is **stubbed** at this freeze (see
> `tools/mai-admin/src/main.rs:1-7`). The corresponding HTTP path
> on a running daemon is `GET /v1/compliance/audit/integrity`;
> the chain-break detection contract this runbook depends on is
> directly exercised by the `test_audit_tamper` test in
> `mai-compliance/tests/compliance_demos.rs`.

## When to use

This runbook handles the **detection** event, not the
verification cadence (that is
[05-verify-audit-chain](05-verify-audit-chain.md)). Triggers:

- `mai-admin audit verify` returned `4`, `5`, or `6`.
- `mai-ship-validate` reports `PROD-AUDIT-100` or `PROD-AUDIT-101`
  failing with chain or signature errors.
- `mai-healthcheck.timer` raised a `audit_chain_break` alert.

This is a **security event**. Treat it as such, even when the
likely root cause is a disk fault. The runbook is the same; the
post-mortem decides intent vs. accident.

## Immediate steps (within 5 minutes of detection)

1. Stop the API service to prevent further appends to a chain
   whose state is now in question:
   ```bash
   sudo systemctl stop mai-api.service
   ```
2. Snapshot both WAL trees as evidence; do not move them:
   ```bash
   STAMP=$(date +%Y%m%d-%H%M%S)
   sudo tar -C /var/lib/mai -czf \
        /var/backups/mai/audit-evidence-$STAMP.tgz \
        audit audit-compliance
   sha3sum /var/backups/mai/audit-evidence-$STAMP.tgz \
        | sudo tee /var/backups/mai/audit-evidence-$STAMP.sha3
   ```
3. Open the incident: [INCIDENT-RESPONSE.md](../INCIDENT-RESPONSE.md).
   This is at minimum a Sev-2.

## Triage (within 1 hour)

1. Rerun the verifier with verbose output to capture the failing
   seq:
   ```bash
   sudo -u mai mai-admin audit verify \
        --wal-dir /var/lib/mai/audit \
        --anchors /etc/mai/trust-anchors \
        --verbose
   ```
   Capture stdout/stderr into the incident record.
2. Identify the failure shape:
   - **Single-seq hash mismatch.** Likely silent disk corruption
     of a specific block; check `smartctl`, `dmesg | grep -i
     ata`, ZFS `zpool status`.
   - **Checkpoint signature failure.** The signing key changed
     mid-window, or the checkpoint segment was overwritten.
     Cross-reference against the anchor rotation log.
   - **Missing range / file truncated.** Operator or attacker
     truncated the file. There is no benign cause; treat as
     intentional until proven otherwise.
3. Cross-check the host: `last`, `journalctl --since "<window>"`,
   shell history of the `mai` and any sudo account, file `mtime`
   on the WAL segments.

## Recovery

Do not "repair" the chain. There is no repair operation; the
chain is the source of truth. Recovery options:

- **Most common.** Restore from the most recent backup that
  predates the tamper window
  ([08-restore-node](08-restore-node.md)). The gap between the
  backup `last_seq` and the broken seq is the lost window;
  document it.
- **If no backup pre-dates the tamper window**, the appliance is
  effectively reset to a pre-MAI state for audit purposes. This
  is a compliance event that exits the runbook and enters
  counsel/regulator-facing process.

## Resumption

After restore:

1. `mai-admin audit verify` must exit 0.
2. `mai-ship-validate` must exit 0.
3. The incident record must include: detection time, evidence
   archive sha3, root cause classification, restore source
   manifest sha3, lost-window bounds, and any regulator
   notifications required by site policy.
4. Only then start `mai-api.service`.

## Do not

- Do not delete WAL segments to "make the verifier happy". This
  is destruction of evidence in addition to being useless.
- Do not run a backup of a tampered tree and treat it as
  recoverable. Backups of broken chains are broken backups.
- Do not skip the incident process because the root cause
  "looks like a disk fault". The audit trail has the same
  legal weight regardless of intent; the process is what
  produces the legally defensible record.
