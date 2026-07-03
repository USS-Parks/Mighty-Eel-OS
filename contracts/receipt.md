# Contract: Receipt (Prompt 0.5)

**Crate:** `fabric-contracts` (type) + `fabric-proof` (chain) · **Status:** v1 (frozen at `contracts-v1`)
**Produced by:** every service · **Consumed by:** `wsf-ledger`, `aog-meter`, console audit search.

## Purpose
The signed, hash-chained record of what happened. Extends the MAI `AuditEntry` (which already carries
BF-5 correlation fields) with the strands WSF/AOG need: which token authorized it, which envelope it
touched, what it spent, and — the capability the cloud cannot offer — which exact model weights
produced an output.

## Wire shape
```json
{
  "receipt_id": "rcp_2026-07-03T18-00-05Z_1b2c",
  "request_id": "req_...",
  "request_hash": "blake3:...",
  "previous_hash": "blake3:...",
  "routing_decision": "LocalOnly",
  "modules_applied": ["hipaa", "destination"],
  "flags": ["phi.detected"],
  "reasons": ["hipaa.min_necessary", "route.local_only_ceiling"],

  "correlation": {
    "credential_event_id": "...", "lamprey_decision_id": "...", "mai_request_id": "...",
    "subject_hash": "hmac:...", "token_id": "tok_...", "tenant_id": "bay-area-pediatrics",
    "bundle_version": "2026.07.03.001", "service_identity": "aog-gateway", "offline_mode": false
  },

  "token_id": "tok_2026-07-03T18-00-00Z_a7f3",
  "envelope_id": "env_... | null",
  "provider": "local:vllm | anthropic | openai | aws-sts | gcp | azure | null",
  "model_weights_digest": "blake3:... | null",
  "spend_cents": 3,
  "tokens_used": 1840,
  "workflow_id": "wf_contract-review_88a1 | null",

  "recorded_at": "2026-07-03T18:00:05Z",
  "periodic_signature": { "alg": "ml-dsa-87", "key_id": "...", "value": "base64...", "covers_through": "rcp_..." }
}
```

## New fields (beyond `AuditEntry`)
| Field | Purpose |
|---|---|
| `receipt_id` | Stable id for this receipt (chain node). |
| `token_id` | The trust token that authorized the action. |
| `envelope_id` | The sealed envelope touched (seal/unseal/label), or null. |
| `provider` | Where inference/creds went (local model, cloud provider, or cloud STS). |
| `model_weights_digest` | For **local** inference: BLAKE3 of the exact weights. The provable-model-identity capability. Null for cloud (they won't disclose it — which is itself audit signal). |
| `spend_cents` / `tokens_used` | Metering inputs for `aog-meter`. |
| `workflow_id` | Ties a multi-call task chain together → cost-per-task, not cost-per-token. |

## Chain + signing
`previous_hash` links receipts into a BLAKE3 chain (fabric-proof). `periodic_signature` is an ML-DSA-87
signature every N receipts, verifiable off-host with the public key only (`fabric-proof::verify_chain`).
Regulated payloads never enter a receipt — only hashes, ids, and metadata (the `TRUST-MANIFOLD.md`
§2.2 boundary holds).

## Verify gate (0.5)
BLAKE3 chain-link test over the extended entry (append N receipts, break one, `verify_chain` flags the
exact index) + an off-host verification test using only the public key.
