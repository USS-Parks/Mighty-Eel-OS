# Runbook 11 — Trust Bundle Expired

> **Status against RC1 freeze (`dceaabc`):** Describes the
> production operator surface. The `mai-admin policy inspect`
> command cited below is **not declared** in the RC1 `mai-admin`
> CLI; bundle inspection at the freeze is via the
> `GET /v1/trust/status` HTTP endpoint or by reading the cached
> bundle file directly. `mai-ship-validate` (shipped binary) does
> implement `PROD-TRUST-100` and can be invoked stand-alone.

## When to use

- `/v1/system/trust/status` reports `bundle_verified = false`
  with reason `expired`.
- `mai-ship-validate` reports `PROD-TRUST-100` failing with
  `bundle expired at <ts>`.
- The daemon is refusing new requests with
  `trust_bundle_expired`.

## Why this happens

The signed policy bundle carries a `not_after` timestamp. Past
that timestamp, the verifier refuses it — by design — even if
the signature still checks out. This is the only way to make
"this signer is no longer trusted to issue *current* policy"
mean something on an air-gapped appliance with no clock-of-record
beyond its own.

## Pre-flight

Confirm the host clock is correct:

```bash
timedatectl
```

A skewed clock can present as a spurious expiry. If the clock
is wrong, fix the clock first; an expired-because-clock-skewed
bundle is **not** a "expired bundle" — it is a host integrity
issue.

## Steps — fresh signed bundle available

1. Obtain the new bundle file + signature off-band.
2. Run [04-install-policy-bundle](04-install-policy-bundle.md)
   end-to-end. The import path itself replaces the cached
   bundle; no manual deletion of the expired one is needed.

## Steps — no fresh bundle yet (degraded mode)

If a fresh bundle cannot be obtained within the service-level
window, the operator must decide: stay fail-closed (no service),
or roll back to the most recent previously-active bundle if it
is still inside its own `not_after` window.

1. List cached bundles:
   ```bash
   sudo ls -lt /var/lib/mai/trust/bundles/
   ```
2. Inspect each candidate's expiry:
   ```bash
   sudo -u mai mai-admin policy inspect \
        /var/lib/mai/trust/bundles/<candidate>.cbor
   ```
3. Pick the most recent bundle whose `not_after` is still in the
   future. If none qualifies, **stop** — see "When no rollback
   is possible" below.
4. Import that bundle via the [04 runbook](04-install-policy-bundle.md).
   The import is itself an audit event; the operator must record
   why the rollback was chosen.

## When no rollback is possible

If every cached bundle is expired, do not bypass the verifier.
The right answer is:

1. Keep the daemon down. It is fail-closed, which is the
   contract.
2. Escalate to the upstream signing authority via the same
   out-of-band channel that delivers bundles. Bundle issuance is
   their responsibility; appliance operators do not have a path
   to re-sign locally, and any feature that allowed it would
   defeat the entire trust model.
3. Communicate downtime to clients.

## Verification

```bash
curl -fsS -H "X-IM-Auth-Token: $MAI_ADMIN_TOKEN" \
     http://127.0.0.1:8420/v1/system/trust/status | jq .
```

Expected: `bundle_verified = true`, `bundle.not_after` in the
future, `signer` matches an installed anchor.

## Do not

- Do not delete the expired bundle from
  `/var/lib/mai/trust/bundles/`. It is evidence; it stays.
- Do not roll the system clock back to make a bundle valid
  again. The audit chain is hash-linked to wall-clock times;
  fiddling the clock corrupts the chain.
- Do not install an "emergency" bundle that did not arrive
  through the documented signed-delivery path. There is no
  emergency hot-path; if the upstream signer cannot reach you,
  the appliance is down until they can.
