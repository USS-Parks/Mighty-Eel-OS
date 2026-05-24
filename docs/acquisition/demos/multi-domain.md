# Demo 4 — Multi-domain Conflict (HIPAA + OCAP)

**Project:** Island Mountain MAI + Lamprey
**Demo target:** Acquirer technical reviewer can run this end to end
in under fifteen minutes and observe the composer's conflict-
resolution behaviour explicitly.
**Status:** Session 45 demo script — RC1.1-docs revision (2026-05-24)
**Companion scaffold:** `apps/compliance-routed/`
**Companion brief:** [`../../LAMPREY-BRIEF.md`](../../LAMPREY-BRIEF.md)
**Backed by automated test:** `test_multi_domain` in
`source/mai-compliance/tests/compliance_demos.rs`

This demo shows what happens when a single request triggers more
than one regulatory module. It exercises the composer's three fold
rules — deny-wins, most-restrictive-route, flag accumulation — and
the OCAP > ITAR > HIPAA precedence chain. The expected outcome is a
single `AggregateDecision` carrying contributions from every module
that fired, with an audit entry that an investigator can walk to see
exactly how the routing decision was reached.

For the higher-level demo catalogue, see
[`../../DEMO-SUITE.md`](../../DEMO-SUITE.md).

---

## Pre-flight

- RC1 bundle unpacked. CWD is `MAI-Lamprey-RC1/`.
- `bin/mai-api.exe` present (RC1 v2) or rustc 1.85+ for source path.
- `curl` available.

---

## Setup

**1. Start the daemon:**

```
.\bin\mai-api.exe
```

**2. Capture and export the first-boot admin key:**

```
export MAI_API_KEY="im-<paste-the-64-hex-key>"          # POSIX
$env:MAI_API_KEY = "im-<paste-the-64-hex-key>"          # PowerShell
```

**3. Activate the TribalGovernment template** (enables both HIPAA
and OCAP — the simplest way to put both modules in play):

```
curl -X POST http://127.0.0.1:8420/v1/compliance/policies/template \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"template":"TribalGovernment"}'
```

**4. Confirm both modules are enabled:**

```
curl http://127.0.0.1:8420/v1/compliance/status \
  -H "X-IM-Auth-Token: $MAI_API_KEY"
```

Expected: `hipaa` and `ocap` modules both `enabled = true`,
ITAR / EAR off.

---

## Trigger

A request that contains PHI *and* carries OCAP-governed metadata:

```
curl -X POST http://127.0.0.1:8420/v1/chat/completions \
  -H "X-IM-Auth-Token: $MAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "lamprey/medical-local",
    "messages": [
      {"role": "user",
       "content": "Patient John Crow (MRN 87654) presented at the tribal clinic with chest pain on 2026-05-22. Recommend imaging?"}
    ],
    "metadata": {
      "ocap": {
        "tribal_source": "nation-x",
        "possession": "tribal_authority",
        "data_class": "community_health",
        "consent": {"cultural": true, "treaty": false}
      }
    }
  }'
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
while HIPAA still allows local-only. Same `curl` as §"Trigger" with
the metadata's `consent.cultural` flipped to `false`.

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
returns `cloud_allowed` (a contrived test setup) and OCAP returns
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

This sub-scenario is exercised primarily via:

```
cd source
cargo test -p mai-compliance policy::composer::most_restrictive_route
```

rather than via the HTTP surface, because crafting a real-world
HIPAA `cloud_allowed` is contrived.

---

## Verification

1. **Query the audit log.**

   ```
   curl "http://127.0.0.1:8420/v1/compliance/audit?tenant=local-dev&limit=5" \
     -H "X-IM-Auth-Token: $MAI_API_KEY"
   ```

   The composite entry includes both modules under `modules[]`, the
   full precedence chain under `precedence_applied`, and the
   composer's explanation array.

2. **Walk the composer test suite.**

   ```
   cd source
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

   ```
   curl -X POST http://127.0.0.1:8420/v1/compliance/reports/generate \
     -H "X-IM-Auth-Token: $MAI_API_KEY" \
     -H "Content-Type: application/json" \
     -d '{"report_type":"SystemActivity"}'
   ```

   The activity report shows the multi-module decision in one row
   with all contributing reasons.

4. **Walk the dashboard Alerts page.**

   The compliance dashboard is a separate FastAPI app under
   `source/compliance-dashboard/`. To run it (requires Python 3.12+
   and the SDK on PYTHONPATH per `README-FIRST.md` §3):

   ```
   cd source/compliance-dashboard
   uvicorn app:app --port 8081
   ```

   Then open `http://127.0.0.1:8081/alerts`. The multi-domain
   decision appears in the live SSE stream with both `ocap` and
   `hipaa` module tags. (Dashboard is not started by `mai-api.exe`;
   it is an optional companion process.)

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
| Dashboard live stream surfaces the multi-module event | Y (if dashboard is running) |

If any check fails, run the automated equivalent first:

```
cd source
cargo test -p mai-compliance --test compliance_demos test_multi_domain -- --nocapture
cargo test -p mai-compliance --lib
```

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
