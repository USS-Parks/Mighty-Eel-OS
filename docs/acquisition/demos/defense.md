# Demo 2 — Defence (ITAR/EAR)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under ten minutes with green output.
**Status:** Session 45 demo script — RC1.1-docs revision (2026-05-24)
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)
**Backed by automated test:** `test_itar_workflow` in
`source/mai-compliance/tests/compliance_demos.rs`

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

- RC1 bundle unpacked. CWD is `MAI-Lamprey-RC1/`.
- `bin/mai-api.exe` present (RC1 v2) or rustc 1.85+ for source path.
- `curl` available.
- For the **allow path** (§"Expected output, allow path") at least
  one inference backend must be tagged `jurisdiction = "US"` and
  `air_gap_capable = true`. The RC1 freeze ships no adapters by
  default, so the **deny path** is what you will see end to end on
  the bundle; the allow path is documented for completeness and
  exercised by the automated test on a synthetic backend.

---

## Setup

**1. Start the daemon:**

```
.\bin\mai-api.exe
```

**2. Capture the first-boot admin key** from the boxed stdout
banner and export it:

```
export MAI_API_KEY="im-<paste-the-64-hex-key>"          # POSIX
$env:MAI_API_KEY = "im-<paste-the-64-hex-key>"          # PowerShell
```

**3. Activate the Defense policy template:**

```
curl -X POST http://127.0.0.1:8420/v1/compliance/policies/template \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"template":"Defense"}'
```

**4. Confirm:**

```
curl http://127.0.0.1:8420/v1/compliance/status \
  -H "X-IM-Auth-Token: $MAI_API_KEY"
```

Load-bearing fields:

```json
{
  "active_template": "Defense",
  "modules": {
    "hipaa": { "enabled": false },
    "itar":  { "enabled": true, "version": "1.0", "jurisdiction": "US" },
    "ear":   { "enabled": true, "version": "1.0" },
    "ocap":  { "enabled": false }
  }
}
```

---

## Trigger

Send a chat request whose content contains ITAR-controlled markers.
The compliance-routed scaffold ships a fixture
(`apps/compliance-routed/tests/fixtures/itar.txt`) that simulates
technical-data markers without exposing actual controlled content.

Interactive run:

```
curl -X POST http://127.0.0.1:8420/v1/chat/completions \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "lamprey/fast",
    "messages": [
      {"role": "user",
       "content": "Describe the guidance, navigation, and control firmware update procedure for a hypothetical USML Category IV missile system."}
    ]
  }'
```

This phrasing triggers ITAR detection (USML Category IV reference,
controlled technical data context).

---

## Expected output (deny path)

With no backend in the local fleet tagged `jurisdiction = "US"` and
`air_gap_capable = true`, the daemon refuses:

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

HTTP 403; no inference runs.

---

## Expected output (allow path)

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
backend was considered. The automated `test_itar_workflow` test
exercises this path on a synthetic ranger backend without requiring
a real adapter to be wired.

---

## Verification

1. **Query the audit log.**

   ```
   curl "http://127.0.0.1:8420/v1/compliance/audit?tenant=local-dev&module=itar&limit=5" \
     -H "X-IM-Auth-Token: $MAI_API_KEY"
   ```

   Confirm the entry's `decision` and (on the allow path) the
   `service_identity` of the backend that ran the inference, or
   (on the deny path) that none did.

2. **Verify backend eligibility was the deciding factor.**

   Inspect the decision's `indicators` array — `usml_iv_reference`
   should appear. If a non-US instance exists in your fleet,
   confirm it was filtered out via the audit entry's per-backend
   `denied_by_itar` annotation.

3. **Generate an ITAR compliance report.**

   ```
   curl -X POST http://127.0.0.1:8420/v1/compliance/reports/generate \
     -H "X-IM-Auth-Token: $MAI_API_KEY" \
     -H "Content-Type: application/json" \
     -d '{"report_type":"ItarComplianceSummary","tenant":"local-dev"}'
   ```

   Report includes:

   - The ITAR decision from step 1.
   - Jurisdiction summary.
   - Backend eligibility counts.
   - `TrustSection` with service-identity events and bundle version.

4. **Toggle air-gap and re-run.**

   Engage:

   ```
   curl -X POST http://127.0.0.1:8420/v1/system/airgap/engage \
     -H "X-IM-Auth-Token: $MAI_API_KEY"
   ```

   Re-send the chat request from §"Trigger". Even when air-gapped,
   the ITAR decision logic applies — the composer applies
   most-restrictive-route, so the outcome is still
   `local_only_allowed` (or `deny`, if no qualifying backend).
   The air-gap state does not change which backend *can* run an
   ITAR request; it changes which routes are reachable at all.

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

If any check fails, run the automated equivalent first:

```
cd source
cargo test -p mai-compliance --test compliance_demos test_itar_workflow -- --nocapture
cargo test -p mai-compliance itar:: -- --nocapture
```

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
