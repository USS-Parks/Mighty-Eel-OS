# Contract: Identity (Prompt 0.3)

**Crate:** `fabric-identity` · **Status:** v1 (frozen at `contracts-v1`)
**Produced by:** `wsf-bridge` (from OpenBao auth events) · **Consumed by:** `fabric-token`, every service.

## Purpose
The stable, signed assertion of *who or what* is acting, before any authority (token) is granted.
Superset-compatible with the MAI claim's subject fields (`TRUST-MANIFOLD.md` §4). Adds first-class
session/task nesting so an agent loop and each tool call carry their own short-lived identity.

## Wire shape
```json
{
  "identity_id": "id_2026-07-03T18-00-00Z_a7f3",
  "kind": "workload",
  "tenant_id": "bay-area-pediatrics",
  "subject_id": "svc:aog-gateway@bayarea-peds.example",
  "subject_hash": "hmac:7c2f3a...",
  "service_identity": "aog-gateway",
  "spiffe_id": "spiffe://islandmountain/tenant/bay-area-pediatrics/aog-gateway",
  "pki_cert_fingerprint": "sha256:9b1c...",
  "parent_id": null,
  "issued_at": "2026-07-03T18:00:00Z",
  "expires_at": "2026-07-03T18:15:00Z",
  "signature": { "alg": "ml-dsa-87", "key_id": "bridge-2026-q3", "value": "base64..." }
}
```

## Fields
| Field | Req | Purpose |
|---|---|---|
| `identity_id` | yes | Globally unique; receipts reference it. |
| `kind` | yes | `human` \| `workload` \| `session` \| `task`. Session/task are short-lived and `parent_id`-linked. |
| `tenant_id` | yes | Unit of policy authority (`TRUST-MANIFOLD.md` §3). |
| `subject_id` | yes | Human-readable identity; logged only via `subject_hash`. |
| `subject_hash` | yes | HMAC-SHA256(per-tenant key, `subject_id`). Audit uses this, never the raw id. |
| `service_identity` | when workload | One of the nine in `SERVICE-IDENTITY.md`; null for humans. |
| `spiffe_id` | yes | SPIFFE-style workload id; the canonical machine identity string. |
| `pki_cert_fingerprint` | yes | Binds to the OpenBao-PKI-issued leaf cert used for mTLS. |
| `parent_id` | session/task only | The identity that spawned this one; enforces the loop → task chain. |
| `issued_at`/`expires_at` | yes | Short window (5–15 min for workload; call-lifetime for task). |
| `signature` | yes | ML-DSA-87 over canonical payload (fabric-proof). |

## Compatibility
An MAI claim's `subject_id`/`subject_hash`/`service_identity`/`tenant_id` map 1:1. A claim without
`spiffe_id`/`kind`/`parent_id` is read as `kind:"human"` (or `workload` when `service_identity` set),
`parent_id:null`. No MAI claim breaks.

## Verify gate (0.3)
serde round-trip test (identity → JSON → identity, byte-stable canonical form) + a test proving an
MAI-shaped subject block deserializes into an `Identity` with the defaults above.
