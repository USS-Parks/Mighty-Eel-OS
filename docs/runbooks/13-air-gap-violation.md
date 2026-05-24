# Runbook 13 — Air-Gap Violation

> **Status against RC1 freeze (`dceaabc`):** Describes the
> production operator surface. The `mai-admin audit tail`
> command cited below is **stubbed** at this freeze (see
> `tools/mai-admin/src/main.rs:1-7`). The corresponding HTTP
> paths on a running daemon are `GET /v1/system/connectivity`
> for the live view and
> `GET /v1/compliance/audit?...` for stored events. The
> `mai-ship-validate` binary (shipped in `bin/`) does implement
> `PROD-NET-100` and can be invoked stand-alone.

## When to use

- `/v1/system/connectivity` reports any non-loopback connection
  attempt out of the `mai-api` process when `air_gap.policy =
  "strict"`.
- An audit entry of class `airgap.violation` lands in the WAL.
- Operator monitoring (e.g. an upstream firewall, host-based
  IDS) reports MAI-originated egress that should be impossible.

## Why this matters

MAI is sold on the contract that regulated payloads do not
leave the local node. The air-gap policy is part of that
contract. A violation — even a benign one (DNS, NTP, a stray
crash reporter) — breaks the demonstrable guarantee. Until the
violation is explained and either justified or eliminated, the
guarantee is suspended.

## Immediate steps

1. Capture the violation entry from the audit feed:
   ```bash
   sudo -u mai mai-admin audit tail --grep airgap.violation -n 50
   ```
   Record the `dst`, `proto`, `process`, `caller`, and `seq`
   fields.
2. If the policy is `strict` and the daemon has not already
   refused service, stop the API while triage runs:
   ```bash
   sudo systemctl stop mai-api.service
   ```
   In `strict` mode the daemon is supposed to fail-closed after
   N violations within a window; if it did not, that is itself a
   second bug — record it and surface to engineering.

## Triage

Classify the destination:

- **`127.0.0.1` / `::1`.** Not a violation; this is loopback.
  The detector likely has a bug.
- **DNS to a configured resolver.** Some host stacks resolve
  hostnames during startup even though MAI itself does not.
  Confirm the resolver is intentional, and that the configured
  policy permits it (it should be listed explicitly in
  `air_gap.allow_loopback_resolvers`).
- **NTP.** Same shape — must be explicit in the allow list.
  Without time sync, audit timestamps drift; with unguarded
  time sync, the air gap is theatre.
- **Anything else.** Treat as compromise until proven otherwise.

For "anything else" the triage flow exits to
[INCIDENT-RESPONSE.md](../INCIDENT-RESPONSE.md).

## Process map

`/v1/system/connectivity` reports the in-process view. To
cross-check at the kernel layer:

```bash
sudo ss -tnp | grep mai-api
sudo lsof -nP -p $(pidof mai-api) | grep -E 'IPv4|IPv6'
```

Both should show only loopback binds in `ship`. If they show
non-loopback binds, the daemon is not in the posture the
profile claims. Stop it and re-validate.

## Recovery

1. Eliminate the source of the violation (config drift, an
   inadvertently installed package's network probe, etc.).
2. Re-run `mai-ship-validate` — `PROD-NET-100` should pass.
3. Start the service.
4. Tail the connectivity feed for one full diurnal cycle and
   confirm no recurrence:
   ```bash
   curl -fsS -H "X-IM-Auth-Token: $MAI_ADMIN_TOKEN" \
        http://127.0.0.1:8420/v1/system/connectivity/events?since=24h
   ```

## Do not

- Do not relax `air_gap.policy` from `strict` to `allow` to
  silence alerts. The right answer is to either justify the
  egress and add it to the explicit allow list, or eliminate
  it. There is no middle ground in `ship`.
- Do not skip the audit-entry classification. Even benign DNS
  queries are evidence of what the daemon's host environment
  permitted; that is data counsel will want.
- Do not assume the violation is benign just because the bytes
  do not look like a payload. Side-channel exfiltration over
  DNS is well-documented; the policy treats *any* egress as
  significant.
