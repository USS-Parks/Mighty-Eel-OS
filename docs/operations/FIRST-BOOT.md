# First Boot

The first boot of a `ship` appliance is the only one that prints
an admin API key in plaintext. Everything that comes after
depends on the operator capturing that key cleanly and persisting
only the hash.

For the on-the-spot procedure, follow runbook
[01-first-boot-and-key-capture](runbooks/01-first-boot-and-key-capture.md).
This document explains the why and the failure surface.

## What "first boot" means

First boot is any boot that finds `/etc/mai/auth_keys.toml`
either absent or with no `[[keys]]` entries. The daemon detects
this, mints a fresh admin key, prints the key + hash banner once
on stdout, and exits before binding any socket.

Subsequent boots find at least one `[[keys]]` entry and skip
the mint step entirely. There is no "re-mint" path — the key
that boots the system is the only key the operator ever sees in
plaintext.

## The banner contract

The banner is a single contiguous block on stdout:

```
========================================
  MAI FIRST-BOOT: Admin API Key
========================================
  Key:  im-<64 hex chars>
  Hash: <64 hex chars>
  Role: admin
  Permissions: *
========================================
```

The banner is emitted *before* the `tracing-subscriber` log
sink reconfigures, which is why it does not appear in JSON log
output and is why step 2 of the runbook pipes through `tee`.

## Why first boot is privileged

Every subsequent operator interaction with the daemon requires
an existing key. The first boot is the only moment where the
daemon will accept "I am the operator" without prior credential
material. The contract is:

1. The first boot must happen on a host whose **physical**
   access is already controlled — the appliance is not yet
   listening on a network port.
2. The banner must be captured in the same console session
   where the daemon is started. No second-window scraping; no
   later journal mining. Either the operator captures it now or
   the key is lost.
3. Once captured, only the **hash** persists to
   `/etc/mai/auth_keys.toml`. The plaintext key never goes to
   disk on the appliance.

## What happens if the key is lost

There is no recovery path. The appliance fail-closes against
its own audit chain, so the operator cannot "just regenerate".
The options are:

1. **Pre-service.** If no client has connected yet, wipe
   `/var/lib/mai/` and `/etc/mai/auth_keys.toml`, then re-run
   the install. There is no audit history to preserve.
2. **Post-service.** If clients have connected and the audit
   chain has live entries, the key is unrecoverable. Restore
   from the most recent backup taken before the key was lost
   (runbook 08), which carries its own key hash. If no such
   backup exists, this is a compliance event — the appliance
   is now unmanageable and must be re-provisioned, with the
   prior audit history archived but inactive.

This is intentional. The alternative — a "recover admin key"
flow — is exactly the back door this design forbids.

## First-boot validator interaction

`mai-ship-validate --profile /etc/mai/profile.toml` run before
first boot will report `PROD-AUTH-100` as the only failing
check (key store empty). That failure is *expected* at this
moment. After step 5 of the runbook (hash persisted, daemon
restarted), the same command must report `PROD-AUTH-100` as
PASS. The check is what closes the first-boot gate.

## Multi-key seeding

Sites that prefer to bootstrap with more than one key (e.g. an
admin key + a separate read-only operator key) can do so by:

1. Completing first boot as above to capture the admin key.
2. Using the admin key to mint additional keys via
   `mai-api keygen --role <role>`.
3. Adding their hashes to `auth_keys.toml`.

There is no batch first-boot. The minting step always involves
a live daemon and the admin credential.

## Audit footprint

First boot writes the following audit entries:

- `system.boot` — process start, version, profile hash.
- `auth.key.minted` — fingerprint of the new key (not the key).
- `auth.key.exit` — the daemon's exit after banner emission.
- `system.boot` — the second start, after the operator
  persisted the hash.

The pair of `system.boot` entries straddling the
`auth.key.minted` is the cryptographic record of the first-boot
event. Auditors look for it; do not delete it under any
circumstance.

## See also

- Runbook [01-first-boot-and-key-capture](runbooks/01-first-boot-and-key-capture.md)
- [SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) — key storage
  policy, rotation cadence.
- [INCIDENT-RESPONSE.md](../compliance/INCIDENT-RESPONSE.md) — when a lost
  key becomes an incident.
