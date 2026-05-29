# Upgrade and Rollback

How a `ship` appliance moves from one MAI release to the next,
and how to back out if the upgrade fails. For the failure path
see [09-recover-from-failed-upgrade](runbooks/09-recover-from-failed-upgrade.md).

## Cadence

Upgrades happen on the operator's schedule. MAI never
auto-upgrades; the daemon does not reach out for updates. The
upgrade artifact (`mai_<version>_amd64.deb`) is delivered the
same way bundles are — out-of-band, signed by the upstream
release signer, verified at the host layer before install.

Typical upgrade triggers:

- Scheduled release (quarterly or per upstream calendar).
- Security fix (out-of-band, may be hot).
- Compliance bundle change that requires a new daemon (rare;
  bundles target a major version).

Do not upgrade just because a release is available. The
appliance is shippable; "newer" is not, on its own, "better".

## Pre-upgrade checklist

Before any upgrade:

1. Verify the running fleet is healthy:
   ```bash
   sudo mai-ship-validate --profile /etc/mai/profile.toml
   sudo systemctl is-active mai-api.service mai-dashboard.service
   sudo -u mai mai-admin audit verify --wal-dir /var/lib/mai/audit
   ```
2. Take a verified backup
   ([07-back-up-node](runbooks/07-back-up-node.md)). Without
   this, rollback is not available; this is the only floor under
   the upgrade.
3. Read the upstream release notes. Specifically: schema
   changes to `profile.toml`, deprecated config keys, new
   `PROD-*` check IDs that will fail closed, and any bundle
   compatibility caveats.
4. Confirm the new package signature against the upstream
   release anchor (operator-side; the appliance does not have a
   release-anchor concept).

## Upgrade procedure

1. Drain client traffic at the reverse proxy. Configurations
   typically use `503` health gating; whatever your proxy
   does, route new requests elsewhere or hold them.
2. Install the new package:
   ```bash
   sudo apt install ./mai_<new-version>_amd64.deb
   ```
   `preinst` stops the services. `postinst` reloads systemd,
   updates the conffile defaults under `/etc/mai/` (operator
   edits are preserved), and prints a next-steps banner.
3. Reconcile config. Read `/etc/mai/profile.toml` against the
   release notes. Any new required sections must be added
   before the next step.
4. Validate against the new binary:
   ```bash
   sudo mai-ship-validate --profile /etc/mai/profile.toml
   ```
   Exit 0 is required. If the validator now fails on a check
   that previously passed, the upgrade is **not yet complete**
   — read the failing check, address the change, re-run.
5. Start the daemon:
   ```bash
   sudo systemctl start mai-api.service
   sudo systemctl status mai-api.service
   curl -fsS http://127.0.0.1:8420/v1/health/ready | jq -r .status
   ```
6. Tail the audit chain for the first minute of post-upgrade
   traffic:
   ```bash
   sudo -u mai mai-admin audit tail -n 50
   ```
7. Take a post-upgrade backup as soon as the daemon serves
   clean traffic. This is the new floor for the next upgrade.
8. Restore client traffic at the reverse proxy.

The upgrade is complete when:

- `mai-ship-validate` exits 0 against the new binary.
- `/v1/health/ready` returns 200.
- The audit chain has advanced and the next `audit verify`
  exits 0.
- A post-upgrade backup has been created and verified.

## Rolling forward across multiple releases

If the appliance is behind by more than one release, **do not
jump versions**. Upgrade through each intermediate release in
sequence. The release-notes contract documents migrations as
"from X.Y to X.Y+1"; skipping versions skips migrations.

The exception is a documented "jump-supported" release pair,
called out explicitly in the release notes. Even then, run the
intermediate `mai-ship-validate` against the source profile
before applying.

## Rollback

Rollback is "re-install the previous package on top of a
verified pre-upgrade backup". See
[09-recover-from-failed-upgrade](runbooks/09-recover-from-failed-upgrade.md)
for the procedure. The short version:

1. Stop the new service.
2. Re-install the previous `.deb` (apt will warn about
   downgrade — confirm).
3. Move the post-upgrade state dir aside (do not delete; it is
   evidence).
4. Restore from the pre-upgrade backup.
5. Validate, start, verify.

Rollback is **always** preferable to "patching forward" when
the upgrade has produced an unrecoverable state. The pre-upgrade
backup exists for exactly this case.

## What upgrades change vs. what they don't

Upgrades change:

- Binaries in `/usr/bin/`, `/usr/lib/mai/`.
- Systemd units in `/lib/systemd/system/mai-*.{service,timer}`.
- Conffile **defaults** under `/etc/mai/` (operator edits are
  preserved by dpkg's conffile handling).
- The `mai-ship-validate` check set — new checks may be added,
  existing checks may be tightened.

Upgrades do **not** change:

- `/var/lib/mai/` state (audit chain, vault, models). The
  daemon is responsible for any schema migration on those, and
  it happens on first start of the new binary, with the
  migration itself recorded as audit events.
- Operator edits in `/etc/mai/`.
- Trust anchors. Anchors live on a different rotation cadence
  than the daemon.

## Boundary

Upgrades are an operator activity. Engineering can ship the
package; only the operator can decide when the appliance is
ready to absorb it. The runbooks tell the operator *how*; the
operator chooses *when*.

## See also

- [INSTALL.md](INSTALL.md) — fresh install.
- [BACKUP-RESTORE.md](BACKUP-RESTORE.md) — the backup that
  makes rollback possible.
- Runbook [07-back-up-node](runbooks/07-back-up-node.md).
- Runbook [09-recover-from-failed-upgrade](runbooks/09-recover-from-failed-upgrade.md).
- [RELEASE-GATES.md](../releases/RELEASE-GATES.md) — what the validator
  checks after the upgrade.
