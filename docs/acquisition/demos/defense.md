# Demo 2 — Defence (ITAR/EAR)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under ten minutes with green output.
**Status:** Session 45 demo script
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)

This demo shows controlled-technical-data detection blocking unsafe
backends. The expected outcome is a `deny` decision (when no
qualifying backend exists) or `local_only_allowed` on a US-only
air-gapped backend, with reason `itar.controlled_data` and a
defence audit entry.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md). For ITAR/EAR module
specifics, see [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)
§"ITAR/EAR module".

---

## Pre-flight

- Repo checked out at the post-S45 commit.
- `mai-api` server can be started.
- `MAI_API_KEY` set to a valid local-dev key with `itar_ear` scope
  attached to the local-dev claim.
- At least one inference backend tagged with
  `jurisdiction = "US"` and `air_gap_capable = true`.

Deployment profile: `mai/deployment/airgap-demo/profile.toml` with
`compliance.template = "Defense"`. This profile is the closest
analogue to a real defence-contractor deployment.

---

## Setup script

```powershell
cd "$env:USERPROFILE\Documents\Claude\Island Mountain Mighty Eel OS\mai"
$env:MAI_DEPLOYMENT_PROFILE = "deployment/airgap-demo"
$env:MAI_COMPLIANCE_TEMPLATE = "Defense"
cargo run --release --bin mai-api

# Second terminal
$env:MAI_API_BASE = "http://127.0.0.1:8080"
$env:MAI_API_KEY  = "im-<airgap-demo-key>"

mai compliance status
```

Expected status:

```json
{
  "active_template": "Defense",
  "modules": {
    "hipaa": { "enabled": false },
    "itar":  { "enabled": true, "version": "1.0", "jurisdiction": "US" },
    "ear":   { "enabled": true, "version": "1.0" },
    "ocap":  { "enabled": false }
  },
  "airgap": { "enabled": true, "switch_engaged": false },
  "trust_mode": "connected"
}
```

---

## Trigger

Send a chat request whose content contains ITAR-controlled markers.
The compliance-routed scaffold ships a fixture (`tests/fixtures/itar.txt`)
that simulates technical-data markers without exposing actual
controlled content.

Interactive run:

```powershell
mai chat "Describe the guidance, navigation, and control firmware update procedure for a hypothetical USML Category IV missile system." --model lamprey/fast
```

This phrasing triggers ITAR detection (USML Category IV reference,
controlled technical data context).

---

## Expected output (deny path)

If no backend in the local fleet is tagged `jurisdiction = "US"`
and `air_gap_capable = true`:

```json
{
  "request_id": "req_<uuid>",
  "decision": {
    "aggregate": "deny",
    "reasons": [
      "itar.controlled_data",
      "itar.no_qualifying_backend"
    ],
    "modules": {
      "itar": { "decision": "deny", "indicators": ["usml_iv_reference"] },
      "ear":  { "decision": "allow" }
    },
    "route_selected": "none"
  },
  "audit_entry_id": 87,
  "http_status": 403,
  "error_code": "MAI-A201"
}
```

The chat call returns HTTP 403; no inference runs.

---

## Expected output (local-only-allowed path)

If a backend is tagged `jurisdiction = "US"` and
`air_gap_capable = true`:

```json
{
  "request_id": "req_<uuid>",
  "decision": {
    "aggregate": "local_only_allowed",
    "reasons": [
      "itar.controlled_data",
      "itar.jurisdiction_satisfied",
      "itar.backend_eligible",
      "trust.scope_check_passed"
    ],
    "modules": {
      "itar": {
        "decision": "local_only",
        "indicators": ["usml_iv_reference"],
        "selected_backend": "ranger-us-001"
      }
    },
    "route_selected": "local_only"
  },
  "audit_entry_id": 87
}
```

Inference runs on the qualifying backend; no cloud or non-US
backend was considered.

---

## Verification steps

1. **Query the audit log.**

   ```powershell
   mai compliance audit --tenant airgap-demo --module itar --limit 5
   ```

   Confirm the entry's `decision` and the `service_identity` of the
   backend that ran the inference (or that none did, in the deny
   path).

2. **Verify backend eligibility was the deciding factor.**

   Inspect the decision's `indicators` array — the `usml_iv_reference`
   indicator should appear. If the backend list contains a non-US
   instance, confirm it was filtered out:

   ```powershell
   mai scheduler instance-metrics ranger-eu-001 | Select-String "denied_by_itar"
   ```

   Counter should be incremented if the eu-001 instance exists.

3. **Generate an ITAR compliance report.**

   ```powershell
   mai compliance report generate ITAR --scope tenant=airgap-demo
   ```

   Report includes:

   - The ITAR decision from step 1.
   - Jurisdiction summary.
   - Backend eligibility counts.
   - `TrustSection` with service-identity events and bundle
     version.

4. **Toggle air-gap and re-run.**

   ```powershell
   # Engage air-gap
   curl -X POST -H "X-IM-Auth-Token: $env:MAI_API_KEY" `
        http://127.0.0.1:8080/v1/system/airgap/engage

   # Re-send the request
   mai chat "<same prompt>" --model lamprey/fast
   ```

   Even when air-gapped, the ITAR decision logic applies — the
   composer applies most-restrictive-route, so the outcome is
   still `local_only_allowed` (or `deny`, if no qualifying
   backend). The air-gap state does not change which backend
   *can* run an ITAR request; it changes which routes are
   reachable at all.

---

## Pass / fail criteria

| Check | Pass |
|---|---|
| ITAR detected via `usml_iv_reference` indicator | Y |
| Composer either denies or selects qualifying backend | Y |
| If allowed: backend has `jurisdiction = "US"` and `air_gap_capable = true` | Y |
| If denied: HTTP 403 with `MAI-A201`, no inference ran | Y |
| Audit entry created with `module = itar` and correlation IDs | Y |
| ITAR report includes jurisdiction summary + TrustSection | Y |

If any check fails, run `cargo test -p mai-compliance itar::` to
confirm the module-level logic is healthy.

---

## What the demo proves

- ITAR module identifies controlled technical data via the
  indicator set (`tech_data.rs`).
- Composer enforces jurisdiction-aware backend eligibility before
  the scheduler is asked to place.
- Air-gap state is a routing input, not a deployment flag — it
  combines with ITAR in the most-restrictive-route fold.
- The defence audit trail is queryable and certifiable, with the
  same chain + PQC signature mechanism as HIPAA and OCAP.
- A non-US backend in the fleet is filtered out without ever being
  asked to run the request; an acquirer can verify this from the
  audit log.

This is the Lamprey value proposition for the defence and dual-use
research market.
