# MAI Operator Runbooks

> **Audience note (RC1.1-docs, 2026-05-24):** These runbooks
> describe the **Linux systemd production deployment** of a MAI
> appliance in `ship` profile. The RC1 tester bundle ships
> **Windows MSVC binaries only**, because the RC1 audience is
> laptop testers (see `README-FIRST.md` §3), not appliance
> operators. Linux appliance binaries arrive in RC2. Until then,
> treat these runbooks as **design documentation** rather than as
> procedures executable on your tester machine — they describe
> what the on-call operator does, not what you do on Windows.
>
> Individual runbooks may also carry a "Status against RC1 freeze"
> band noting which `mai-admin` subcommands they reference are
> stubbed or undeclared at the freeze; in those cases the HTTP
> equivalents on a running daemon are the operative surface for
> RC1 testers.

Concise, single-purpose procedures for operating a MAI appliance
in `ship` profile. Every runbook follows the same shape:

- When to use
- Preconditions
- Steps
- Verification
- Do-not list

Runbooks are intended for the operator on call at 2 AM. They
are not architecture docs and do not explain *why*; for that,
follow the cross-links into [SHIP-PROFILE.md](../operations/SHIP-PROFILE.md),
[SECURITY.md](../compliance/SECURITY.md), or the relevant
[architecture/](../architecture/) chapter.

## Index

| # | Runbook | Typical trigger |
|---|---|---|
| 01 | [First boot and key capture](01-first-boot-and-key-capture.md) | New appliance |
| 02 | [Rotate API key](02-rotate-api-key.md) | Scheduled or compromise |
| 03 | [Rotate trust anchor](03-rotate-trust-anchor.md) | Anchor expiry or compromise |
| 04 | [Install a new policy bundle](04-install-policy-bundle.md) | Lamprey bundle update |
| 05 | [Verify audit chain](05-verify-audit-chain.md) | Scheduled, pre-backup |
| 06 | [Generate a compliance report](06-generate-compliance-report.md) | Attestation, counsel request |
| 07 | [Back up a node](07-back-up-node.md) | Scheduled, pre-change |
| 08 | [Restore a node from backup](08-restore-node.md) | Bare-metal recovery |
| 09 | [Recover from a failed upgrade](09-recover-from-failed-upgrade.md) | Upgrade rollback |
| 10 | [Adapter crash loop](10-adapter-crash-loop.md) | Adapter `failed` state |
| 11 | [Trust bundle expired](11-trust-bundle-expired.md) | `PROD-TRUST-100` fail |
| 12 | [Audit WAL tamper detected](12-audit-wal-tamper.md) | `audit verify` non-zero |
| 13 | [Air-gap violation](13-air-gap-violation.md) | `airgap.violation` audit entry |
| 14 | [Disk almost full](14-disk-almost-full.md) | `disk_low` alert |

## Cross-cutting docs

- [INSTALL.md](../operations/INSTALL.md) — full install + first-boot flow.
- [BACKUP-RESTORE.md](../operations/BACKUP-RESTORE.md) — backup/restore
  contract, retention, DR drill matrix.
- [UPGRADE-ROLLBACK.md](../operations/UPGRADE-ROLLBACK.md) — upgrade
  protocol; runbook 09 is the failure path.
- [INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md) — severity
  classification, communications, post-mortem template.
- [OBSERVABILITY.md](../operations/OBSERVABILITY.md) — metrics, health
  endpoints, alert rules feeding these runbooks.
- [SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) — key
  rotation, key storage, anchor hygiene.
- [TRUST-BRIDGE-PRODUCTION.md](../compliance/TRUST-BRIDGE-PRODUCTION.md) —
  bundle delivery, signing key custody.
- [AUDIT-RETENTION.md](../compliance/AUDIT-RETENTION.md) — retention
  policy and operator-side archival.
- [RELEASE-GATES.md](../releases/RELEASE-GATES.md) — what the validator
  checks; how runbooks restore each gate.

## Profile boundary

Every runbook above is for `profile.mode = "production"` only.
For `local-dev`, `airgap-demo`, `local-mai-node`, or
`cloud-trust-core` see [SHIP-PROFILE.md](../operations/SHIP-PROFILE.md);
this directory does not document non-ship behavior.
