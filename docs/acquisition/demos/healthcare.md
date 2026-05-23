# Demo 1 — Healthcare (HIPAA)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under ten minutes with green output.
**Status:** Session 45 demo script
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)

This demo shows PHI detection forcing a local-only route on a chat
request that contains protected health information. The expected
outcome is `local_only_allowed` with reason `hipaa.phi_detected`,
plus a HIPAA audit entry that an operator can later turn into a
compliance report.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md).

---

## Pre-flight

- Repo checked out at the post-S45 commit.
- `mai-api` server can be started (no live OpenBao required —
  `local-dev` deployment profile works).
- `MAI_API_KEY` environment variable set to a valid local-dev key.
- Python 3.11+ with `mai-sdk-python` installed (`pip install -e
  mai-sdk-python/`).

Deployment profile: `mai/deployment/local-dev/profile.toml` with
`compliance.template = "Healthcare"`.

---

## Setup script

```powershell
# 1. Start mai-api with the Healthcare template
cd "$env:USERPROFILE\Documents\Claude\Island Mountain Mighty Eel OS\mai"
$env:MAI_DEPLOYMENT_PROFILE = "deployment/local-dev"
$env:MAI_COMPLIANCE_TEMPLATE = "Healthcare"
cargo run --release --bin mai-api

# 2. In a second terminal, set the API key for the SDK
$env:MAI_API_BASE = "http://127.0.0.1:8080"
$env:MAI_API_KEY  = "im-<your-local-dev-key>"

# 3. Confirm the Healthcare template is active
mai compliance status
```

Expected `mai compliance status` output:

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

---

## Trigger

Send a chat request whose content contains a PHI marker. The
scaffold ships a fixture; for an interactive run:

```powershell
mai chat "Patient John Doe (MRN 123456) presented with chest pain on 2026-05-22. Recommend imaging?" --model lamprey/medical-local
```

The fixture markers in `apps/compliance-routed/tests/fixtures/`
exercise:

- Patient name + MRN (HIPAA identifier #1, #2)
- Date of service (HIPAA identifier #3)
- Diagnosis context

---

## Expected output

The chat response itself runs locally and returns model output. The
*decision metadata* is what proves the demo:

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
and the chat completion runs on a locally-loaded model. No prompt
content crossed the cloud trust boundary.

---

## Verification steps

1. **Query the audit log for the recorded decision.**

   ```powershell
   mai compliance audit --tenant local-dev --module hipaa --limit 5
   ```

   The newly-recorded entry appears with `decision = local_only_allowed`,
   `module = hipaa`, and full `CorrelationFields` (credential_event_id,
   lamprey_decision_id, mai_request_id).

2. **Verify the audit chain integrity.**

   ```powershell
   curl -H "X-IM-Auth-Token: $env:MAI_API_KEY" `
        http://127.0.0.1:8080/v1/compliance/audit/integrity | ConvertFrom-Json
   ```

   Returns `{ "chain_status": "intact", "head_id": 42, "verified_count": 42 }`.

3. **Generate a HIPAA compliance report.**

   ```powershell
   mai compliance report generate HIPAA --scope tenant=local-dev
   ```

   Returns a report ID. The report includes:

   - The PHI decision from step 1.
   - A `TrustSection` (§A.13) with trust mode and bundle version.
   - A summary count of decisions per route.

4. **Download the certified report.**

   ```powershell
   mai compliance report download <id> --format json --out hipaa-report.json
   ```

   The file's `signature_hex` field can be re-verified off-host
   with `verify_certified_report` from `mai-compliance::reports`.

---

## Pass / fail criteria

| Check | Pass |
|---|---|
| Decision is `local_only_allowed` | Y |
| `hipaa.phi_detected` is in reasons | Y |
| Inference returned a response (locally) | Y |
| Audit entry created with HIPAA module + correlation IDs | Y |
| Audit chain integrity is `intact` | Y |
| HIPAA report generated with `TrustSection` | Y |
| Report's signature verifies via `verify_certified_report` | Y |

If any check fails, run `pytest apps/compliance-routed/tests/`
first to confirm the local-dev scaffold is healthy; the scaffold
exercises the same path under controlled fixtures.

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
