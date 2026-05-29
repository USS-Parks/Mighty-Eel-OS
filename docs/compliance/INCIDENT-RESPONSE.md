# Incident Response

The operator-facing process for handling MAI incidents. The
runbooks under [runbooks/](runbooks/) handle specific named
failures; this document is for the cases where the failure is
unknown, or where the named failure has unfolded into a
multi-system event.

## Severity classes

| Sev | Definition | Examples | First action |
|---|---|---|---|
| Sev-1 | Confidentiality breach plausible; audit chain compromised; air-gap broken; trust bundle forged | Tamper detected on WAL; live air-gap egress to unknown destination; bundle signed by uninstalled anchor accepted | Stop the daemon. Preserve evidence. Page security lead. |
| Sev-2 | Production traffic affected; signing keys lost; backup integrity in doubt | Adapter crash loop across all models; backup verify fails on the only verified backup; trust anchor inadvertently deleted | Drain traffic. Page on-call. |
| Sev-3 | Single feature degraded; recoverable with documented runbook | Single adapter failed; bundle expiring soon; disk approaching full | Run the relevant runbook. |
| Sev-4 | Cosmetic / alert tuning | Spurious 401 spike from a known-bad client; metric noise | Investigate in business hours. |

Promote a Sev-3 to a Sev-2 if you find the documented runbook
does not match what the appliance is doing. Stale runbooks are
an engineering bug; communicate it.

## Universal first-five-minutes

Regardless of class, in the first five minutes:

1. **Acknowledge.** Whoever is on call states (in the operator
   channel) "I am on it, this is potentially Sev-X."
2. **Stabilize, do not investigate.** If client traffic is
   affected, drain at the reverse proxy. If the audit chain is
   in question, stop the daemon. Investigation comes after the
   bleeding stops.
3. **Preserve evidence.** Snapshot whatever caused the alarm
   before changing anything. Examples:
   ```bash
   STAMP=$(date +%Y%m%d-%H%M%S)
   sudo tar -C /var/lib/mai -czf \
        /var/backups/mai/incident-$STAMP.tgz audit audit-compliance
   sudo journalctl --since "1h ago" > /tmp/journal-$STAMP.log
   curl -fsS -H "X-IM-Auth-Token: $T" \
        http://127.0.0.1:8420/v1/system/production-readiness \
        > /tmp/readiness-$STAMP.json
   ```
4. **Open the record.** Start an incident document with: who,
   when, what triggered, what was changed. Update as you go.
5. **Page the right people.** Sev-1 wakes security. Sev-2 wakes
   the on-call engineer. Sev-3 stays with the operator until
   resolved or escalated.

## Communication

The MAI appliance never talks to incident-tracking systems on
its own. Communication is the operator's responsibility.
Suggested cadence:

| Sev | Internal cadence | External cadence |
|---|---|---|
| Sev-1 | Every 15 min until contained | Counsel notified within 1 hr; regulator per site policy |
| Sev-2 | Every 30 min until resolved | Customer-facing once traffic restored |
| Sev-3 | At resolution only | None unless customer asked |
| Sev-4 | Per business hours | None |

For Sev-1 incidents touching the audit chain, treat counsel
notification as a hard requirement — the chain's evidentiary
value depends on the chain-of-custody being recorded in real
time, not reconstructed after the fact.

## Investigation flow

Once stabilized:

1. **Triage** — which subsystem? Use the alert -> runbook map
   in [OBSERVABILITY.md](../operations/OBSERVABILITY.md). If the symptom does
   not match a runbook, the incident is genuinely novel; treat
   it as such.
2. **Hypothesize, then test, then change.** Do not change
   config or restart services in the hope something improves.
   Every change widens the post-mortem.
3. **One change at a time.** Each change is logged in the
   incident record with a timestamp. If the change does not
   help, **revert it** before trying the next one.
4. **Verify recovery.** A recovered appliance has:
   - `mai-ship-validate` exit 0.
   - `/v1/health/ready` 200.
   - `audit verify` exit 0.
   - A fresh post-incident backup, verified.
   - The alert that fired has cleared and stayed cleared for
     at least one full alert evaluation window.

## Post-mortem

Every Sev-1 and Sev-2 gets a written post-mortem within five
business days. Sev-3s get one if the runbook proved insufficient
or if anything was learned that should change a runbook.

The post-mortem covers:

- Timeline: detection, response, stabilization, recovery.
- Root cause and contributing factors.
- What worked and what did not.
- Evidence pointers (incident bundle sha3, journal capture,
  WAL evidence archive).
- Action items, each with an owner and a due date.

Blameless framing. The point of the post-mortem is to make the
next incident shorter, not to find someone to scold.

## Specific incident shapes

### Audit chain compromised

Runbook [12-audit-wal-tamper](runbooks/12-audit-wal-tamper.md)
is the entry point. Always at least Sev-2; promote to Sev-1
if the cause is anything other than disk fault and the post-
mortem cannot exclude intentional tampering.

### Air-gap violation

Runbook [13-air-gap-violation](runbooks/13-air-gap-violation.md).
At least Sev-2. Always Sev-1 if the destination is unknown or
the bytes egressed were anything other than a clearly-benign
classified protocol (DNS / NTP that should have been on the
allow list).

### Trust bundle forged

If the daemon accepts a bundle signed by an anchor not
installed in `/etc/mai/trust-anchors/`, that is a Sev-1 by
definition. The validator should refuse this; if it did not,
the validator itself is the bug.

### Backup integrity in doubt

The most recent verified backup is the floor under recovery.
Losing that floor is a Sev-2 — even if the daemon is healthy,
the lack of a recoverable backup is itself the incident.

### Compliance signing key lost

Quarterly reports cannot be signed. The appliance is operable;
the operator's compliance posture is degraded. Sev-2, exits
the appliance immediately into operator key-management.

## Boundary

Incident response is operator-led, not automated. The appliance
emits signals; the operator decides what they mean. A daemon
that decides on its own when to "restart for safety" or "drop
to read-only mode" is a daemon that takes decisions that
counsel needs to make. The contract is: MAI reports; the
operator acts.

## See also

- [OBSERVABILITY.md](../operations/OBSERVABILITY.md) — signals and alerts.
- [SECURITY-PRODUCTION.md](SECURITY-PRODUCTION.md) — posture
  the appliance enforces.
- [BACKUP-RESTORE.md](../operations/BACKUP-RESTORE.md) — recovery
  primitives.
- [runbooks/](runbooks/) — named-failure procedures.
