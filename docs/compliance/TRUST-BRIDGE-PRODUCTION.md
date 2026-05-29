# Trust Bridge in Production

The trust bridge is the operator's side of the contract between
the upstream signing authority and the appliance. It is *not* a
running service component — there is no "Trust Bridge daemon"
inside `mai-api`. The bridge is the procedural shape: how
bundles arrive, who signs them, who installs them, who verifies.

For the cryptographic primitives see
[TRUST-BUNDLE-SPEC.md](TRUST-BUNDLE-SPEC.md). For the runbook
that installs a bundle see
[04-install-policy-bundle](runbooks/04-install-policy-bundle.md).
For the wider architecture see
[TRUST-MANIFOLD.md](TRUST-MANIFOLD.md).

## Roles

| Role | Held by | Responsibility |
|---|---|---|
| Bundle author | Upstream policy team | Writes Lamprey policy modules |
| Bundle signer | Upstream signing authority (HSM) | Signs bundle + records bundle id |
| Bundle delivery | Operator-chosen courier | Out-of-band, signed-channel delivery |
| Anchor custodian | Operator | Holds `.pub` material, rotates per policy |
| Appliance verifier | `mai-api` daemon | Verifies bundle signature against installed anchors |
| Audit witness | Daemon + dashboard | Records every import event in the WAL |

No single role can both author and install in production. That
separation is the trust bridge.

## What a bundle is

A bundle is a single CBOR file plus a sibling `.sig`:

- `<bundle-id>.bundle.cbor` — policy modules, rule definitions,
  enforcement profile, `not_before` / `not_after`, signer
  identifier.
- `<bundle-id>.bundle.cbor.sig` — ML-DSA-87 signature over the
  bundle bytes, produced by the upstream signing key.

Bundle IDs are operator-friendly (`2026-05-23`,
`hipaa-2026-q2`). The signature determines authority; the ID is
naming.

## Delivery shape

Bundles arrive at the appliance host out-of-band. The accepted
shapes:

1. **Sneakernet** — operator carries the bundle on signed media,
   verifies the courier's chain of custody on arrival.
2. **Signed channel** — bundle file is itself wrapped in a
   second-factor signature delivered over a separate channel
   (encrypted email, SFTP into an operator-controlled jump
   host). The double-signature does not increase the security
   of the bundle itself — it increases the security of the
   *delivery*.
3. **Air-gapped pull from a mirror** — for sites with an
   operator-managed mirror that the appliance can reach (which
   in `ship` means the air-gap policy must explicitly permit
   the mirror endpoint in `air_gap.allow_egress`).

Whichever shape, the appliance verifies the same way: signature
against installed anchors at import time.

## Why the bridge is procedural, not service-side

Earlier iterations of the design contemplated an in-fleet
"Trust Bridge" service that brokered bundles between authoring
and delivery. That design was rejected because it made the
in-fleet service a single point of compromise — exactly the
property the manifold architecture exists to avoid. The bridge
is procedural specifically so that no in-fleet component can
be compromised into accepting an unsigned bundle.

The architectural counterpart is in
[TRUST-MANIFOLD.md](TRUST-MANIFOLD.md): the manifold puts
multiple appliances at the same trust posture without a single
service mediating them.

## Anchor distribution

Trust anchors (the `.pub` material) ride a different channel
from bundles. The reasoning is the same as the four-key
custody matrix in
[SECURITY-PRODUCTION.md](SECURITY-PRODUCTION.md): if the same
courier carries both the bundle and the anchor that signs it,
the courier is the trust root, not the signing authority.

Anchor delivery shapes:

1. Operator pre-stages anchors during install (the install
   procedure does exactly this).
2. New anchors arrive out-of-band, on different media than the
   bundles they sign.
3. Fingerprint cross-check is the operator's job. The appliance
   loads what is in `/etc/mai/trust-anchors/` and verifies; it
   does **not** look up fingerprints anywhere.

## Verification chain at import

When the operator runs
[`mai-admin policy import`](runbooks/04-install-policy-bundle.md):

1. The CLI reads bundle + sig.
2. It loads every `.pub` from `/etc/mai/trust-anchors/`.
3. It verifies the signature against each anchor; one match is
   enough.
4. On a clean dry-run it parses the bundle schema, checks
   `not_before <= now < not_after`, and computes a diff against
   the active bundle.
5. On apply, it atomically rotates the active bundle in the
   policy runtime and records the event in the audit WAL.

If steps 1–4 produce any error, the bundle is rejected. There
is no override; there is no `--force`. The runbook explains why.

## Bundle revocation

Revocation is rare and explicit. The shape:

1. Upstream issues a new bundle that supersedes the revoked
   one and ships with an earlier `not_after` on the revoked
   bundle's id.
2. Operator imports the new bundle via the standard runbook.
3. The daemon, on next bundle check, sees the revoked bundle
   is past its (lowered) `not_after` and refuses to use it.

There is no "delete this bundle" hot path. The audit chain
needs to record that the bundle existed and when it stopped
being trusted; deletion would forge that record.

## Per-site customization

Sites that need a per-site policy module that the upstream
authority does not author have two choices:

1. **Co-signed bundle.** The upstream authority signs a bundle
   that includes the site-specific modules. This keeps a single
   custody chain.
2. **Layered bundle.** The site runs its own signing authority
   with its own anchor installed alongside the upstream one,
   and ships a separate bundle layered on top. This requires
   two anchors and two bundle imports, and the layered design
   is documented in [TRUST-MANIFOLD.md](TRUST-MANIFOLD.md).

There is no third option in `ship`. Hand-edited policy files
in `/etc/mai/policies/` are rejected by the validator.

## See also

- [TRUST-BUNDLE-SPEC.md](TRUST-BUNDLE-SPEC.md) — bundle file
  format and CBOR schema.
- [TRUST-MANIFOLD.md](TRUST-MANIFOLD.md) — multi-appliance
  trust architecture.
- [SECURITY-PRODUCTION.md](SECURITY-PRODUCTION.md) — key
  custody matrix.
- [04-install-policy-bundle](runbooks/04-install-policy-bundle.md),
  [03-rotate-trust-anchor](runbooks/03-rotate-trust-anchor.md),
  [11-trust-bundle-expired](runbooks/11-trust-bundle-expired.md).
