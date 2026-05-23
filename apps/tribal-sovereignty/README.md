# Tribal Data Sovereignty Demo

Tribal communities that own data about their members have the right to
determine where that data is processed, who can access it, and under
what governance conditions. This scaffold demonstrates that MAI can
honor those requirements architecturally: not through a configuration
flag, but through route and model guards that enforce the community's
governance claim before any inference call is made.

The governing framework is OCAP: Ownership, Control, Access, and
Possession. The First Nations Information Governance Centre defines
Possession as the mechanism through which Ownership is protected. If a
community does not physically control its data, it cannot meaningfully
own it. This scaffold enforces Possession at the application layer:
data governed by an OCAP-scoped claim cannot route to a cloud backend.
The route guard refuses before the SDK call would even happen. No
exception path, no fallback to a less-restrictive route, no silent
allow.

The full OCAP pipeline in `mai-compliance` is the server-side
enforcement layer this scaffold is designed to run against. It includes
nine decision stages, with cultural consent and treaty consent as
distinct gates.

---

## What This Enforces

### Possession: Local-Only Route Enforcement

`guard_route(claim, intended_route)` checks that the intended route is
present in `claim.allowed_routes`. For a claim with
`compliance_scopes=["ocap"]`, the trust bridge sets
`allowed_routes=["local_only"]`. Any attempt to route to a cloud backend
raises `SovereigntyViolation` immediately. The SDK call is never made,
and the data never moves.

This is not a post-hoc filter. The refusal happens at the governance
layer before inference, with an auditable reason attached.

### Access: Model Governance

`guard_model(claim, model)` checks that the requested model is in
`claim.allowed_models`. A community governing its health data may permit
a local clinical model and prohibit general-purpose models that could
expose sensitive patterns. The claim encodes that decision; the guard
enforces it without requiring the application to implement its own
access logic.

### Audit: Governance Metadata On Every Request

Every request that passes the guards logs the full governance context:
`tenant_id`, `subject_id`, `subject_hash`, `compliance_scopes`,
`service_identity`, and `trust_bundle_version`. The `claim_id` is the
join key that links each inference event to the governance decision that
authorized it. When the Lamprey audit layer correlates by `claim_id`,
the chain from governance authority to inference outcome is complete
and tamper-evident.

---

## Run

Prerequisites: `mai-api` running on port 8420 and `MAI_API_KEY` set.

```powershell
# Local-only route: this should succeed.
python apps/tribal-sovereignty/main.py "Summarize the health intake form" --dry-run

# Cloud route: this should refuse with exit code 4.
python apps/tribal-sovereignty/main.py "Summarize the health intake form" `
  --intended-route cloud_allowed --dry-run
```

The refusal path is the proof. Exit code 4 with a
`SovereigntyViolation` error confirms that the governance constraint is
enforced before the SDK reaches any backend. The data's locality was
never at risk.

---

## How This Connects To The Full OCAP Pipeline

The guards in this scaffold, `guard_route` and `guard_model`, enforce
Possession and Access at the application layer. The full server-side
evaluation in `mai-compliance::OcapEvaluator` runs nine stages before a
route decision reaches the application:

1. Scope check
2. Revocation check
3. Trust local-only ceiling
4. Possession evaluation
5. Control evaluation
6. Sacred role gate
7. Elder role gate
8. Cultural consent gate
9. Treaty consent gate

When the application calls into the live policy runtime, `guard_route`
becomes:

```python
decision = client.compliance.decide(metadata)
if decision.route not in claim.allowed_routes:
    raise SovereigntyViolation(...)
```

The application logic does not change. The SDK shape was designed
against the same `AggregateDecision` the policy runtime emits, so the
governance enforcement deepens from two-stage application guards to the
nine-stage server-side pipeline without changing the calling code.

The OCAP compliance report includes possession status, consent status,
and treaty consent for every governed request. Every field is
independently auditable.

---

## Tests

```powershell
pytest apps/tribal-sovereignty/tests/ -v
```

`test_smoke.py` proves the guards accept authorized routes and models
and refuse unauthorized ones; confirms the protected corpus loads under
the correct directory; confirms `--dry-run` makes no SDK calls. These
tests establish that governance enforcement is present and fail-closed
before any network path is exercised.

`test_integration.py` proves the full pipeline: governance claim issued,
guards pass, inference call made, response returned. It includes the
refusal path, confirming that when the guard fails, no SDK call is made
and the error carries the correct `SovereigntyViolation` shape.
