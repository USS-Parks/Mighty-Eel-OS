# Demo 3 — Tribal Data Sovereignty (OCAP)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under fifteen minutes with green output.
**Status:** Session 45 demo script
**Companion scaffold:** `apps/tribal-sovereignty/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)

This demo shows the OCAP nine-stage decision pipeline evaluating a
request that carries tribal-data metadata. The expected outcome is
either `local_only_allowed` (when consent + possession + scope are
satisfied) or a typed `OcapError` refusal at the failing stage,
with the full sovereignty decision recorded for tribal review.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md). For OCAP module
specifics, see [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)
§"OCAP module".

---

## Pre-flight

- Repo checked out at the post-S45 commit.
- `mai-api` server can be started.
- `MAI_API_KEY` set to a local-dev key with `ocap` scope attached
  to the local-dev claim.
- A tribal-governance role (`elder` or `cultural_steward`) attached
  for the consent-required paths.

Deployment profile: `mai/deployment/local-mai-node/profile.toml`
with `compliance.template = "TribalGovernment"`.

---

## Setup script

```powershell
cd "$env:USERPROFILE\Documents\Claude\Island Mountain Mighty Eel OS\mai"
$env:MAI_DEPLOYMENT_PROFILE = "deployment/local-mai-node"
$env:MAI_COMPLIANCE_TEMPLATE = "TribalGovernment"
cargo run --release --bin mai-api

# Second terminal
$env:MAI_API_BASE = "http://127.0.0.1:8080"
$env:MAI_API_KEY  = "im-<local-mai-node-key>"

mai compliance status
```

Expected status:

```json
{
  "active_template": "TribalGovernment",
  "modules": {
    "ocap":  { "enabled": true, "version": "1.2" },
    "hipaa": { "enabled": true, "version": "1.4" },
    "itar":  { "enabled": false },
    "ear":   { "enabled": false }
  },
  "trust_mode": "connected"
}
```

---

## Trigger

The tribal-sovereignty scaffold ships fixtures for three sovereignty
scenarios. Interactive runs:

### Scenario 3a — Authorised consent path

```powershell
mai chat "Summarise the storage protocol for our community health survey data." `
  --model lamprey/fast `
  --metadata '{"ocap":{"tribal_source":"nation-x","possession":"tribal_authority","data_class":"community_health","consent":{"cultural":true,"treaty":false}}}'
```

Expected: `local_only_allowed` with reasons
`ocap.scope_check_passed`, `ocap.possession_verified`,
`ocap.cultural_consent_satisfied`.

### Scenario 3b — Missing cultural consent

Same prompt with `consent.cultural = false`. Expected:
`OcapError::CulturalConsentMissing`, HTTP 403, no inference.

### Scenario 3c — Sacred data with non-elder role

```powershell
mai chat "<prompt referencing sacred ceremonial data>" `
  --model lamprey/fast `
  --metadata '{"ocap":{"tribal_source":"nation-x","possession":"tribal_authority","data_class":"sacred","consent":{"cultural":true}}}'
```

Expected: if the subject lacks the `sacred_role` permission,
`OcapError::SacredRoleRequired` at stage 6.

---

## Expected output (authorised consent path, scenario 3a)

```json
{
  "request_id": "req_<uuid>",
  "decision": {
    "aggregate": "local_only_allowed",
    "reasons": [
      "ocap.scope_check_passed",
      "ocap.no_revocation",
      "ocap.local_only_ceiling_passed",
      "ocap.possession_verified",
      "ocap.control_authorised",
      "ocap.sacred_role_not_required",
      "ocap.elder_role_not_required",
      "ocap.cultural_consent_satisfied",
      "ocap.treaty_consent_not_required"
    ],
    "modules": {
      "ocap": {
        "decision": "local_only",
        "pipeline_stages_completed": 9,
        "tribal_source": "nation-x",
        "possession": "tribal_authority"
      }
    },
    "route_selected": "local_only"
  },
  "audit_entry_id": 142
}
```

Notice all nine pipeline stages are recorded — sovereignty review
boards can see exactly which gates were passed and which were
skipped.

---

## Verification steps

1. **Query the audit log for the OCAP decision.**

   ```powershell
   mai compliance audit --tenant local-mai-node --module ocap --limit 5
   ```

   The entry includes the full nine-stage trace plus the
   `service_identity` of the local instance that ran the inference.

2. **Re-run with missing consent (scenario 3b) and confirm refusal.**

   ```powershell
   mai chat "<same prompt>" --metadata '{"ocap":{...,"consent":{"cultural":false}}}'
   ```

   Returns HTTP 403 with `MAI-A201`, reason
   `ocap.cultural_consent_missing`. Audit entry records the
   refusal at stage 8.

3. **Run the scaffold test suite end-to-end.**

   ```powershell
   PYTHONPATH=mai-sdk-python/src python -m pytest apps/tribal-sovereignty/tests/ -v
   ```

   Should report 9 green tests including
   `test_sovereignty_violation_when_authority_mismatch`.

4. **Generate an OCAP compliance report.**

   ```powershell
   mai compliance report generate OCAP --scope tenant=local-mai-node
   ```

   The report includes:

   - Per-stage decision counts (how often each gate refused or
     passed).
   - Possession and consent summary.
   - Treaty consent events.
   - `TrustSection` with bundle version and policy version.

---

## Pass / fail criteria

| Check | Pass |
|---|---|
| Scenario 3a: `local_only_allowed`, nine stages completed | Y |
| Scenario 3b: HTTP 403, refusal at stage 8 (cultural consent) | Y |
| Scenario 3c: HTTP 403, refusal at stage 6 (sacred role) | Y |
| All three audit entries carry full correlation fields | Y |
| `pytest apps/tribal-sovereignty/tests/` reports 9 green | Y |
| OCAP report renders with per-stage counts + TrustSection | Y |
| The decision *never* defaults to allow on missing scope | Y |

If any check fails, run `cargo test -p mai-compliance ocap::` to
confirm the module-level pipeline is healthy.

---

## What the demo proves

- OCAP is a real pipeline, not a keyword filter. Every stage has a
  typed error and a reason code.
- Fail-closed semantics: missing scope refuses; this is verifiable
  by reading `OcapEvaluator::evaluate` and the
  `OcapError::ScopeMissing` variant.
- Tribal data sovereignty rules are evaluated at inference time,
  before placement, and recorded for sovereignty audit.
- Cultural and treaty consent are distinct gates with distinct
  reason codes — a tribal authority can audit each independently.
- The OCAP report is the artefact a tribal data governance board
  can review without trusting the underlying source code (signed,
  re-verifiable).

This is the Lamprey value proposition for tribal health, tribal
energy, and treaty-land deployments — and the most uniquely
defensible piece of the acquisition IP.
