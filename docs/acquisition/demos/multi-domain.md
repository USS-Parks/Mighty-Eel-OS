# Demo 4 — Multi-domain Conflict (HIPAA + OCAP)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under fifteen minutes and observe the composer's conflict-
resolution behaviour explicitly.
**Status:** Session 45 demo script
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)

This demo shows what happens when a single request triggers more
than one regulatory module. It exercises the composer's three
fold rules — deny-wins, most-restrictive-route, flag accumulation —
and the OCAP > ITAR > HIPAA precedence chain. The expected outcome
is a single `AggregateDecision` carrying contributions from every
module that fired, with an audit entry that an investigator can
walk to see exactly how the routing decision was reached.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md).

---

## Pre-flight

- Repo checked out at the post-S45 commit.
- `mai-api` server running.
- `MAI_API_KEY` set to a key with both `hipaa` and `ocap` scopes
  attached.

Deployment profile: `mai/deployment/local-mai-node/profile.toml`
with a *custom* template that enables both HIPAA and OCAP:

```toml
[compliance]
template = "Custom"
modules  = ["hipaa", "ocap"]
```

The shipped `TribalGovernment` template enables both, so:

```powershell
$env:MAI_COMPLIANCE_TEMPLATE = "TribalGovernment"
```

is the simplest way to set up. The relevant property is that *both*
modules return non-trivial decisions on the same request.

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

Expected: `hipaa` and `ocap` modules both enabled. ITAR / EAR off.

---

## Trigger

A request that contains PHI *and* carries OCAP-governed metadata:

```powershell
mai chat "Patient John Crow (MRN 87654) presented at the tribal clinic with chest pain on 2026-05-22. Recommend imaging?" `
  --model lamprey/medical-local `
  --metadata '{"ocap":{"tribal_source":"nation-x","possession":"tribal_authority","data_class":"community_health","consent":{"cultural":true,"treaty":false}}}'
```

This triggers:

- **HIPAA module:** patient name + MRN + service date → PHI
  detected.
- **OCAP module:** tribal-source data with possession verified and
  cultural consent → local-only allowed.

The composer must fold these into one decision.

---

## Expected output

```json
{
  "request_id": "req_<uuid>",
  "decision": {
    "aggregate": "local_only_allowed",
    "reasons": [
      "ocap.scope_check_passed",
      "ocap.possession_verified",
      "ocap.cultural_consent_satisfied",
      "hipaa.phi_detected",
      "hipaa.minimum_necessary_satisfied",
      "composer.most_restrictive_route_applied",
      "composer.precedence_ocap_over_hipaa"
    ],
    "modules": {
      "ocap": {
        "decision": "local_only",
        "headline_reason": "ocap.possession_and_consent_satisfied"
      },
      "hipaa": {
        "decision": "local_only",
        "detected_entities": ["patient_name", "mrn", "service_date"]
      }
    },
    "route_selected": "local_only",
    "precedence_applied": ["ocap", "hipaa"],
    "explanation": [
      "OCAP module returned local_only (possession + consent satisfied).",
      "HIPAA module returned local_only (PHI detected).",
      "Composer applied most-restrictive-route fold: both modules agree on local_only.",
      "Precedence chain (OCAP > HIPAA) used for headline reason; HIPAA reasons carried as secondary."
    ]
  },
  "audit_entry_id": 201
}
```

The decision is `local_only_allowed`; the explanation array
documents the composer's reasoning step by step.

---

## Sub-scenario A — Deny-wins

To exercise deny-wins, remove cultural consent so OCAP refuses
while HIPAA still allows local-only:

```powershell
mai chat "<same prompt>" `
  --metadata '{"ocap":{...,"consent":{"cultural":false}}}'
```

Expected:

```json
{
  "decision": {
    "aggregate": "deny",
    "reasons": [
      "ocap.cultural_consent_missing",
      "hipaa.phi_detected",
      "composer.deny_wins"
    ],
    "modules": {
      "ocap":  { "decision": "deny" },
      "hipaa": { "decision": "local_only" }
    },
    "route_selected": "none"
  }
}
```

HTTP 403. Inference does not run. OCAP's `deny` propagated through
the composer; HIPAA's `local_only` was *not* able to override.

---

## Sub-scenario B — Most-restrictive-route

To exercise the route fold, configure a custom template where HIPAA
returns `cloud_allowed` (in a contrived test setup) and OCAP returns
`local_only`. The composer downgrades to `local_only`:

```json
{
  "decision": {
    "aggregate": "local_only_allowed",
    "reasons": [
      "ocap.possession_and_consent_satisfied",
      "hipaa.phi_not_detected",
      "composer.most_restrictive_route_applied"
    ],
    "modules": {
      "ocap":  { "decision": "local_only" },
      "hipaa": { "decision": "cloud_allowed" }
    },
    "route_selected": "local_only"
  }
}
```

This sub-scenario is exercised primarily via
`cargo test -p mai-compliance policy::composer::most_restrictive_route`
rather than the SDK, because crafting a real-world HIPAA
`cloud_allowed` is contrived.

---

## Verification steps

1. **Query the audit log.**

   ```powershell
   mai compliance audit --tenant local-mai-node --limit 5
   ```

   The composite entry includes both modules under
   `modules[]`, the full precedence chain under
   `precedence_applied`, and the composer's explanation array.

2. **Walk the composer test suite.**

   ```powershell
   cargo test -p mai-compliance policy::composer -- --nocapture
   ```

   Tests cover:

   - `test_deny_wins_across_modules`
   - `test_most_restrictive_route_fold`
   - `test_flag_accumulation`
   - `test_precedence_chain_ocap_over_hipaa`
   - `test_precedence_chain_ocap_over_itar`
   - `test_module_versions_recorded`

3. **Re-render the audit entry as a report.**

   ```powershell
   mai compliance report generate SystemActivity --scope request_id=<id>
   ```

   The activity report shows the multi-module decision in one row
   with all contributing reasons.

4. **Walk the dashboard Alerts page.**

   Open `http://127.0.0.1:8081/alerts` (default dashboard port).
   The multi-domain decision appears in the live SSE stream with
   both `ocap` and `hipaa` module tags.

---

## Pass / fail criteria

| Check | Pass |
|---|---|
| Both modules fire on a single request | Y |
| Composer fold rule applied is named in the explanation | Y |
| Precedence chain used for headline reason is recorded | Y |
| Deny-wins sub-scenario refuses with `MAI-A201` | Y |
| Most-restrictive-route sub-scenario downgrades route | Y |
| All composer tests green | Y |
| Audit entry carries module versions for every contributor | Y |
| Dashboard live stream surfaces the multi-module event | Y |

If any check fails, run `cargo test -p mai-compliance --lib` to
confirm the composer logic is healthy.

---

## What the demo proves

- The composer is not a sequential chain — it folds module
  decisions explicitly using deny-wins, most-restrictive-route,
  and flag accumulation.
- Precedence is configurable per deployment, with sensible
  defaults (OCAP > ITAR > HIPAA) that reflect remediability
  difficulty.
- Every contributing module appears in the audit entry with its
  version, so a future regulator can replay the decision against
  the historical module code.
- The explanation array makes the routing logic legible to an
  investigator without source-code access.
- The HTTP and SDK surfaces report the aggregate decision plus
  the contributors — clients can branch on module-specific
  reasons.

This is the Lamprey value proposition for regulated organisations
that touch *more than one* regulatory domain — which is most of
them. Healthcare in tribal jurisdictions, defence contractors with
tribal-land facilities, federally-funded research on Indigenous
populations, and many more cross-domain customer profiles all live
in the multi-module decision space.
