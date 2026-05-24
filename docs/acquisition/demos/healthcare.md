# Demo 1 — Healthcare (HIPAA)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under ten minutes with green output.
**Status:** Session 45 demo script — RC1.1-docs revision (2026-05-24)
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)
**Backed by automated test:** `test_hipaa_workflow` in
`source/mai-compliance/tests/compliance_demos.rs`

This demo shows PHI detection forcing a local-only route on a chat
request that contains protected health information. The expected
outcome is `local_only_allowed` with reason `hipaa.phi_detected`,
plus a HIPAA audit entry that an operator can later turn into a
compliance report.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md).

---

## Pre-flight

- RC1 bundle unpacked. Commands below assume your CWD is the
  bundle root (`MAI-Lamprey-RC1/`).
- For the binary path (RC1 v2): `bin/mai-api.exe` is present. No
  toolchain required.
- For the source path: rustc 1.85+ available.
- `curl` available (Windows 10+ ships it; on PowerShell you can
  also use `Invoke-WebRequest`).

This demo exercises the live HTTP surface. The compliance engine
itself — PHI detection, BAA enforcement, composer fold, audit
chain, certified report — is independently verified by the
`test_hipaa_workflow` test, which runs as part of
`cargo test -p mai-compliance --test compliance_demos` (covered
in `TESTER-INSTRUCTIONS.md` §4.B).

---

## Setup

**1. Start the daemon** (RC1 v2 binary path):

```
.\bin\mai-api.exe
```

Or source path: `cd source && cargo run --release --bin mai-api`.

**2. Capture the first-boot admin key** from the boxed stdout
banner (one line that starts with `im-` followed by 64 hex
characters). Export it into your second shell:

```powershell
$env:MAI_API_KEY = "im-<paste-the-64-hex-key-here>"
```

```bash
export MAI_API_KEY="im-<paste-the-64-hex-key-here>"
```

**3. Activate the Healthcare policy template** so the HIPAA
module fires on this demo's traffic:

```powershell
curl.exe -X POST http://127.0.0.1:8420/v1/compliance/policies/template `
  -H "X-IM-Auth-Token: $env:MAI_API_KEY" `
  -H "Content-Type: application/json" `
  -d '{"template":"Healthcare"}'
```

```bash
curl -X POST http://127.0.0.1:8420/v1/compliance/policies/template \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"template":"Healthcare"}'
```

**4. Confirm the template is active:**

```
curl http://127.0.0.1:8420/v1/compliance/status \
  -H "X-IM-Auth-Token: $MAI_API_KEY"
```

Expected JSON shape:

```json
{
  "active_template": "Healthcare",
  "modules": {
    "hipaa": { "enabled": true, "version": "1.4" },
    "itar":  { "enabled": false },
    "ear":   { "enabled": false },
    "ocap":  { "enabled": false }
  },
  "policy_version": "...",
  "trust_mode": "connected"
}
```

Exact field set may evolve; the load-bearing field for this demo
is `modules.hipaa.enabled = true`.

---

## Trigger

Send a chat request whose content contains a PHI marker:

```
curl -X POST http://127.0.0.1:8420/v1/chat/completions \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "lamprey/medical-local",
    "messages": [
      {"role": "user",
       "content": "Patient John Doe (MRN 123456) presented with chest pain on 2026-05-22. Recommend imaging?"}
    ]
  }'
```

The fixture markers in `apps/compliance-routed/tests/fixtures/`
exercise:

- Patient name + MRN (HIPAA identifier #1, #2)
- Date of service (HIPAA identifier #3)
- Diagnosis context

---

## Expected output

The chat response itself runs locally and returns model output (if
an adapter is wired) or a `no adapters configured` envelope if not.
The *decision metadata* is what proves the demo, and it is recorded
to the audit log whether or not an adapter ran. The decision shape:

```json
{
  "request_id": "req_<uuid>",
  "decision": {
    "aggregate": "local_only_allowed",
    "reasons": [
      "hipaa.phi_detected",
      "hipaa.minimum_necessary_satisfied",
      "trust.scope_check_passed"
    ],
    "modules": {
      "hipaa": {
        "decision": "local_only",
        "detected_entities": ["patient_name", "mrn", "service_date"]
      }
    },
    "route_selected": "local_only"
  },
  "audit_entry_id": 42
}
```

The decision is `local_only_allowed`, the routing is `local_only`,
and no prompt content crossed the cloud trust boundary.

---

## Verification

1. **Query the audit log for the recorded decision.**

   ```
   curl "http://127.0.0.1:8420/v1/compliance/audit?tenant=local-dev&module=hipaa&limit=5" \
     -H "X-IM-Auth-Token: $MAI_API_KEY"
   ```

   The newly-recorded entry appears with `decision = local_only_allowed`,
   `module = hipaa`, and full `CorrelationFields`
   (`credential_event_id`, `lamprey_decision_id`, `mai_request_id`).

2. **Verify the audit chain integrity.**

   ```
   curl http://127.0.0.1:8420/v1/compliance/audit/integrity \
     -H "X-IM-Auth-Token: $MAI_API_KEY"
   ```

   Returns `{ "chain_status": "intact", "head_id": 42, "verified_count": 42 }`.

3. **Generate a HIPAA compliance report.**

   ```
   curl -X POST http://127.0.0.1:8420/v1/compliance/reports/generate \
     -H "X-IM-Auth-Token: $MAI_API_KEY" \
     -H "Content-Type: application/json" \
     -d '{"report_type":"HipaaAuditTrail","tenant":"local-dev"}'
   ```

   Returns a report ID. The generated report includes the PHI
   decision from step 1, a `TrustSection` (§A.13) with trust mode
   and bundle version, and per-route summary counts.

4. **Download the certified report.**

   ```
   curl "http://127.0.0.1:8420/v1/compliance/reports/<id>/download?format=json" \
     -H "X-IM-Auth-Token: $MAI_API_KEY" \
     -o hipaa-report.json
   ```

   The file's `signature_hex` field can be re-verified off-host
   with `verify_certified_report` from `mai-compliance::reports`.

---

## Pass / fail criteria

| Check | Pass |
|---|---|
| Decision is `local_only_allowed` | Y |
| `hipaa.phi_detected` is in reasons | Y |
| Audit entry created with HIPAA module + correlation IDs | Y |
| Audit chain integrity is `intact` | Y |
| HIPAA report generated with `TrustSection` | Y |
| Report's signature verifies via `verify_certified_report` | Y |

If any check fails, run the automated equivalent first:

```
cd source
cargo test -p mai-compliance --test compliance_demos test_hipaa_workflow -- --nocapture
```

A green test there isolates the failure to the live HTTP path
rather than the compliance engine itself.

---

## What the demo proves

- HIPAA module fires on inference-time PHI markers (not after-the-fact).
- Composer turns the module output into a local-only route, not a
  generic "warning."
- The scheduler never considered cloud placement; the decision
  preceded placement.
- The audit log records the decision with correlation IDs the
  acquirer can join into their own SIEM.
- The HIPAA report is signed and re-verifiable without trusting
  Island Mountain source code.

This is the Lamprey value proposition for the healthcare market in
one ten-minute run.
