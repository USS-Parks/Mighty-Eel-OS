# Runbook 05 — Verify Audit Chain

> **Status against RC1 freeze (`dceaabc`):** Describes the
> production operator surface. The `mai-admin audit verify`
> command cited below is **stubbed** at this freeze (see header
> comment in `tools/mai-admin/src/main.rs:1-7`). RC1 ships
> `mai-admin backup {create,verify}` and `mai-admin restore
> {plan,apply}`; the `audit` top-level variant exits with a
> "pending session" message. The corresponding HTTP path on a
> running daemon is `GET /v1/compliance/audit/integrity` (per
> `mai-api/src/routes.rs:243`).

## When to use

- Scheduled integrity check (recommended weekly).
- Pre-backup verification (the chain is replayed on every
  `mai-admin backup verify` anyway, but a standalone run lets you
  isolate failures from backup mechanics).
- Post-incident — after any unexpected restart, disk-full event,
  or `journalctl` warning mentioning `audit`.

## What "verify" means

The MAI audit WAL is an append-only JSON-lines file with a
hash-chained tail and periodic ML-DSA-87 checkpoint signatures.
Verifying the chain replays every entry, recomputes the rolling
hash, and confirms each checkpoint signature against the trust
anchors. Any mismatch means corruption, tampering, or a missing
range. None of those are silent.

## Steps

1. Run the standalone verifier:
   ```bash
   sudo -u mai mai-admin audit verify \
        --wal-dir /var/lib/mai/audit \
        --anchors /etc/mai/trust-anchors
   ```
   Exit codes:
   - `0` chain fully verified.
   - `4` chain broken (hash mismatch on a specific seq).
   - `5` checkpoint signature failed.
   - `6` missing range / file truncated mid-segment.
2. On success, the command prints:
   ```
   entries: 124538
   checkpoints: 26 verified
   last_seq: 124538
   last_hash: <hex>
   verified_at: 2026-05-23T19:14:02Z
   ```
3. Run the same verification against the compliance audit WAL:
   ```bash
   sudo -u mai mai-admin audit verify \
        --wal-dir /var/lib/mai/audit-compliance \
        --anchors /etc/mai/trust-anchors
   ```

## On failure — escalate, do not patch

A failed verification is a security event. The daemon already
records the failure in its own audit feed; do not delete or
overwrite the WAL.

1. Stop the API service to prevent further appends until the
   incident is triaged:
   ```bash
   sudo systemctl stop mai-api.service
   ```
2. Snapshot the WAL dir as evidence:
   ```bash
   sudo tar -C /var/lib/mai -czf \
        /var/backups/mai/audit-evidence-$(date +%Y%m%d-%H%M).tgz \
        audit audit-compliance
   ```
3. Open an incident per
   [INCIDENT-RESPONSE.md](../INCIDENT-RESPONSE.md). See also
   [12-audit-wal-tamper](12-audit-wal-tamper.md).

## Verification cadence

- Daily: handled by `mai-healthcheck.timer`; failure raises an
  alert (see [OBSERVABILITY.md](../OBSERVABILITY.md)).
- Weekly: manual full-chain verify as documented above. Record
  the `last_seq` and `last_hash` in your operator log.
- Pre-backup: implicit, by `mai-admin backup verify`.

## Do not

- Do not run `audit verify` while the API is appending without
  understanding that the tail seq may move; the verifier accepts
  this and reports the seq it stopped at.
- Do not edit the WAL by hand. Ever. There is no recovery path
  from a hand-edited WAL except restore-from-backup.
- Do not extend retention by copying WAL segments to another path
  and merging — the chain is the source of truth, copies break it.
