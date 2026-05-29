# Production Security Posture

Operator-facing security contract for a `ship` appliance. For
the cryptographic primitives and trust-bundle internals see
[SECURITY.md](SECURITY.md) and
[TRUST-BUNDLE-SPEC.md](TRUST-BUNDLE-SPEC.md). For non-production
posture see [SHIP-PROFILE.md](../operations/SHIP-PROFILE.md).

## What ship promises

In `profile.mode = "production"`, the daemon refuses to bind
sockets unless every check below holds. The checks are
encoded in `mai-ship-validate`; this document describes the
**why** for each.

1. **Real vault backend.** `StubVault` is rejected. The
   reference deployment uses ZFS with encryption-at-rest. The
   master key for vault content lives sealed under TPM, never
   plaintext on disk.
2. **Persistent audit WAL with PQC checkpoints.**
   `MemoryAuditWriter` and `NullSealer` are rejected. The WAL
   is hash-chained, periodically signed by ML-DSA-87, and
   sealed with AEAD before flush.
3. **ML-DSA bundle verifier with on-disk anchors.**
   `AcceptAllBundleVerifier` and synthetic local-dev token
   exchange are rejected. At least one `.pub` anchor must live
   under `/etc/mai/trust-anchors/` and the active bundle must
   verify against it.
4. **Non-empty API key store with hash-only persistence.** The
   internal-profile-header bypass is rejected; clients must
   present an `X-IM-Auth-Token` that matches a stored hash.
5. **Loopback bind.** TLS terminates at a reverse proxy; the
   daemon itself never offers a public listener.
6. **Air-gap policy.** Default `strict`. Any in-process egress
   attempt that is not on the explicit allow-list is logged
   and refused.

The `ship` profile is the **only** profile that makes those
promises. Sites that run `airgap-demo` for demos or
`local-mai-node` for integration must not claim production
posture.

## Key custody matrix

Four cryptographic keys interact with the appliance. Each has
its own custody chain.

| Key | Purpose | Storage on appliance | Storage off-appliance |
|---|---|---|---|
| Admin API key | Operator + admin endpoints | Hash only, `/etc/mai/auth_keys.toml` | Plaintext copy in operator's key vault |
| Audit AEAD master key | Seals audit WAL at rest | TPM-sealed, vault under `/var/lib/mai/vault/` | None (regenerated from sealed material) |
| Policy bundle signer | Signs Lamprey bundles | Not on appliance | HSM at the bundle authoring site |
| Backup manifest signer | Signs `manifest.json` | Not on appliance | HSM at the operator's backup-signing site |
| Compliance report signer | Signs quarterly reports | Not on appliance | HSM at the operator's compliance-signing site |
| Trust anchors | Verifies bundles | Public material under `/etc/mai/trust-anchors/` | Source of truth at the signing authority |

The four "off-appliance" rows are deliberate. An appliance
that can sign its own bundles, its own backups, and its own
compliance reports is not a trust anchor; it is a circle.

## Rotation cadence

| Material | Cadence | Procedure |
|---|---|---|
| Admin API keys | Quarterly | [02-rotate-api-key](runbooks/02-rotate-api-key.md) |
| Client API keys | Per site policy | same runbook, client-issued |
| Trust anchors | Annually, or on compromise | [03-rotate-trust-anchor](runbooks/03-rotate-trust-anchor.md) |
| Policy bundle signer | Per upstream policy | Off-appliance; bundle reissued |
| Backup manifest signer | Annually, or on compromise | Operator key-management |
| Compliance report signer | Annually, or on compromise | Operator key-management |
| Audit AEAD master key | On any TPM reset; otherwise stable | Vault rekey procedure |

Rotation is additive by default — install the new material,
verify it carries the workload, retire the old material. The
emergency path (replace immediately) only runs when compromise
is known or suspected.

## Reverse proxy contract

MAI binds `127.0.0.1:8420` (REST) and `127.0.0.1:50051` (gRPC).
The reverse proxy in front:

1. Terminates TLS using site-managed certificates. The
   appliance does not ship cert material; the host operator
   chooses how to provision it.
2. Forwards `X-IM-Auth-Token` verbatim. The daemon authenticates
   against the hash; the proxy never inspects the token value.
3. Forwards `Host` and `X-Forwarded-For`. The audit chain
   records both.
4. Does not buffer SSE/WS streams. Disabling output buffering on
   the chosen proxy is a hard requirement; the inference SSE
   surface produces tokens at sub-100ms intervals.
5. Enforces site rate limits in front of MAI's own per-key
   limits. The daemon's limits are a backstop, not the primary
   throttle.

Operator configs for nginx and Caddy live under
[`deployment/`](../deployment/).

## Network posture

The only ports the daemon opens are `127.0.0.1:8420` and
`127.0.0.1:50051`. The `mai-dashboard` unit also binds loopback
on a separate port (default `8421`). The healthcheck timer is
a unit-local exec; it opens no listener.

`PROD-NET-100` enforces this. Any attempt to set `network.bind`
to a non-loopback address in the profile fails the validator.

## In-process egress

`/v1/system/connectivity` is the in-process view of egress
attempts. In `strict`, the only permitted destinations are the
ones listed in `air_gap.allow_loopback_resolvers` (DNS) and
`air_gap.allow_egress` (NTP, configured update endpoints). Both
default to empty.

Anything outside the allow-list is logged as
`airgap.violation` and refused. See
[13-air-gap-violation](runbooks/13-air-gap-violation.md).

## Boundary

Production security posture is not a checklist; it is the
intersection of the appliance contract above, the operator's
key custody, the upstream signing authority, and the site's
own change-management. Each layer fails open without the
others. The appliance is responsible only for the part it can
enforce — the rest is operator discipline.

## See also

- [SHIP-PROFILE.md](../operations/SHIP-PROFILE.md) — per-section profile
  contract.
- [SECURITY.md](SECURITY.md) — cryptographic primitives,
  trust model, threat surface.
- [TRUST-BRIDGE-PRODUCTION.md](TRUST-BRIDGE-PRODUCTION.md) —
  bundle delivery and signing-key custody.
- [INCIDENT-RESPONSE.md](INCIDENT-RESPONSE.md) — when a
  posture check fails open.
