# Audit Retention

What the appliance retains, for how long, where it lives, and
the operator's responsibilities around archival and counsel-
facing requests.

For the audit chain primitives see
[SECURITY.md](SECURITY.md) and
[AUDIT-CORRELATION.md](AUDIT-CORRELATION.md); for the runbooks
that act on the chain see
[05-verify-audit-chain](runbooks/05-verify-audit-chain.md),
[06-generate-compliance-report](runbooks/06-generate-compliance-report.md),
and [12-audit-wal-tamper](runbooks/12-audit-wal-tamper.md).

## Two chains

The appliance maintains two append-only hash-chained WALs:

1. **API audit WAL** at `/var/lib/mai/audit/`. Records every
   API request the daemon evaluated, the route, the auth
   subject, the policy decision, the response classification,
   and the latency band. Does **not** record request or
   response payload content.
2. **Compliance audit WAL** at `/var/lib/mai/audit-compliance/`.
   Records every policy evaluation event the Lamprey composer
   produced, including module-level outcomes and conflict
   resolution traces. Same content boundary — no payload.

Both chains share the same writer contract: JSON-lines,
sha3-256 rolling hash, periodic ML-DSA-87 checkpoint signature,
AEAD-sealed at rest.

## What is recorded

For every chain entry:

- `seq` — monotonic sequence number; never resets across boots.
- `ts_unix_nanos` — wall-clock timestamp.
- `kind` — event class (`api.request`, `auth.key.minted`,
  `policy.decision`, `system.boot`, etc.).
- `subject` — auth subject (key fingerprint, role).
- `route` / `module` — the API route or policy module
  involved.
- `decision` — outcome class (`allow`, `redact`, `block`,
  `error`).
- `prev_hash` — chain link to the previous entry.
- `entry_hash` — sha3-256 of the entry content + `prev_hash`.

Every Nth entry (default 1024) carries an additional
`checkpoint_sig` field: an ML-DSA-87 signature over the entry
hash, signed by the daemon's audit signing key, which itself
chains back to the installed trust anchors.

What is **not** recorded:

- Request body content.
- Response body content.
- Any client-supplied PHI / ITAR / OCAP-protected material.
- Auth tokens themselves (only the hash fingerprint).

This is a hard contract enforced at the writer; the schema
rejects fields outside the allow-list.

## Default retention

The chain itself is append-only and is not deleted by MAI.
Operators decide when to archive segments. Defaults documented
in `/etc/mai/audit-retention.toml`:

| Tier | Window | Action |
|---|---|---|
| Live | Most recent 90 days | On-disk, hot |
| Warm | 90 days – 1 year | On-disk, optional offline copy |
| Cold | 1 year – 7 years | Operator archives via the export procedure below |
| Indefinite | > 7 years | Operator-decided, per site policy |

The 7-year default is the floor for HIPAA-style retention. ITAR
and OCAP sites typically require longer; consult counsel.

The daemon never deletes audit segments. The pruner that
maintains `/var/lib/mai/audit/` only ever **rotates** segments
into archive directories; deletion is an operator action,
documented and audited at the site layer.

## Export procedure

To hand a window of audit history to a counsel or auditor:

1. Verify the chain end-to-end:
   ```bash
   sudo -u mai mai-admin audit verify \
        --wal-dir /var/lib/mai/audit \
        --anchors /etc/mai/trust-anchors
   ```
2. Export the window:
   ```bash
   sudo -u mai mai-admin audit export \
        --wal-dir /var/lib/mai/audit \
        --start 2026-01-01T00:00:00Z \
        --end   2026-03-31T23:59:59Z \
        --out   /var/lib/mai/reports/audit-q1-2026.ndjson \
        --sign
   ```
   `--sign` produces a sidecar `.sig` signed by the compliance
   report signer. Always sign exports.
3. Cross-check the export contains the expected `last_seq`
   range.
4. Deliver via the same out-of-band channel used for
   compliance reports.

The export is a **read** of the chain; it does not change the
chain. The chain still lives on the appliance.

## Cold-storage archival

When the operator decides to move segments out of the hot
window:

1. Run `mai-admin audit verify` against the full chain — never
   archive an unverified chain.
2. Run `mai-admin audit archive --before <ts> --out <dir>`.
   The command writes a signed manifest covering the archived
   segments and leaves a sealed pointer in the live tree
   recording what was moved.
3. Verify the archive directory against itself.
4. Move the archive to cold storage. The pointer in the live
   tree stays; deleting it would forge the chain.

The live chain remains continuous because the rotated segment's
hash is in the next live segment's `prev_hash`. Archival
breaks neither the chain nor the verification; it only changes
where the bytes live.

## Counsel-facing requests

When counsel or a regulator asks for "the audit log":

1. Bound the request to a window with a `start` and `end`
   timestamp. "Everything" is rarely what they want.
2. Run the verify-then-export sequence above.
3. Hand off the signed `.ndjson` + `.sig` pair, plus the
   `last_seq` bounds and the trust anchor fingerprints active
   during the window.
4. Keep a copy. Counsel-facing exports are themselves audited
   events at the operator's site layer.

Do not ever:

- Export an unsigned audit window for counsel.
- Hand off only the WAL files without the manifest signatures.
- Re-encode the audit content into a different schema "for
  readability". The chain is the record; anything else is a
  derivative work and counsel must be told as much.

## Retention vs. backup

These are different. **Backups** capture a snapshot of state
that can be restored to recreate a working appliance.
**Retention** preserves the audit chain so the *history* of
the appliance is not lost. A backup contains a snapshot of the
audit WAL; an archive of audit segments is not a backup of the
appliance.

When in doubt: backups are for recovery; retention is for
attestation.

## See also

- Runbook [05-verify-audit-chain](runbooks/05-verify-audit-chain.md).
- Runbook [12-audit-wal-tamper](runbooks/12-audit-wal-tamper.md).
- [AUDIT-CORRELATION.md](AUDIT-CORRELATION.md) — correlating
  API and compliance audit events.
- [BACKUP-RESTORE.md](../operations/BACKUP-RESTORE.md) — retention vs.
  backup boundary.
