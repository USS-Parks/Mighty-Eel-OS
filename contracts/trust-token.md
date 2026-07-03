# Contract: Trust Token (Prompt 0.4)

**Crate:** `fabric-token` · **Status:** v1 (frozen at `contracts-v1`)
**Produced by:** `wsf-bridge` · **Consumed by:** `wsf-broker`, `aog-gateway`, `aog-toolproxy`, `wsf-seal`.

## Purpose
The WSF primitive. A trust token = the MAI `SignedClaim` (all payload fields) **plus** two new
strands: a **budget** (spend ceiling carried in the same object finance and security both reason
about) and **attenuation** (a token can mint a narrower child — biscuit/macaroon-style). It is a
wire-compatible **superset** of the MAI claim: an existing claim is a token with no budget and no
attenuation.

## Wire shape
```json
{
  "token_id": "tok_2026-07-03T18-00-00Z_a7f3",
  "issued_at": "2026-07-03T18:00:00Z",
  "expires_at": "2026-07-03T18:15:00Z",
  "issuer": "wsf-bridge",
  "trust_bundle_version": "2026.07.03.001",

  "tenant_id": "bay-area-pediatrics",
  "subject_hash": "hmac:7c2f3a...",
  "service_identity": "aog-gateway",
  "identity_id": "id_2026-07-03T18-00-00Z_a7f3",

  "roles": ["clinician"],
  "compliance_scopes": ["hipaa"],
  "allowed_routes": ["local_only"],
  "allowed_models": ["llama-3-8b-instruct"],
  "max_data_classification": "restricted",
  "country": "US",
  "person_type": "us_person",
  "offline_mode": false,
  "revocation_status": "unknown",

  "budget": {
    "token_cap": 200000, "tokens_spent": 0,
    "usd_cap_cents": 500, "usd_spent_cents": 0,
    "tool_call_cap": 25, "tool_calls_spent": 0
  },
  "attenuation": {
    "parent_id": null,
    "caveats": []
  },

  "signature": { "alg": "ml-dsa-87", "key_id": "bridge-2026-q3", "value": "base64..." }
}
```

## New fields (beyond the MAI claim)
| Field | Purpose |
|---|---|
| `token_id` | Renamed `claim_id`; receipts/broker exchanges key on it. (`claim_id` accepted as an alias on read.) |
| `identity_id` | Links to the `fabric-identity` this token was granted to. |
| `budget.*` | Spend ceilings + running counters. Atomic decrement on use; over-cap = deny. **Absent `budget` = enforcement off** (legacy-claim compatibility); the bridge always populates it for new tokens. |
| `attenuation.parent_id` | The token this was minted from (null = root). |
| `attenuation.caveats[]` | Narrowing predicates: `{type,value}` where `type ∈ {route_ceiling, model_allowlist, resource_prefix, tool_allowlist, expiry_before, classification_ceiling}`. |

## Attenuation rule (the hard invariant)
A child token is valid only if **every** caveat narrows (never widens) the parent:
- `allowed_routes(child) ⊆ allowed_routes(parent)` (and never above the parent's ceiling)
- `allowed_models(child) ⊆ allowed_models(parent)`
- `max_data_classification(child) ≤ parent`
- `budget(child) ≤ parent.remaining` for every counter
- `expires_at(child) ≤ parent.expires_at`
Fail-closed: a caveat the verifier does not understand ⇒ reject.

## Compatibility
An MAI claim (no `budget`, no `attenuation`, `claim_id` not `token_id`) deserializes as a root token
with budget-enforcement off. Signed-payload canonicalization is unchanged (fabric-proof
`write_canonical`), so existing signatures still verify.

## Verify gate (0.4)
Round-trip test + a test proving an old-shape MAI claim deserializes as a token with empty
budget/caveats and still passes signature verification; an attenuation test proving a child cannot
exceed parent scope/budget on any axis.
