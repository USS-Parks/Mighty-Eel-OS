# Contract: Envelope (Prompt 0.6)

**Crate:** `fabric-envelope` · **Status:** v1 (frozen at `contracts-v1`)
**Produced by:** `wsf-seal`, `aog-gateway` (egress tokenization) · **Consumed by:** anything moving regulated data.

## Purpose
Naked regulated data never travels. Every finding, sample, prompt span, or classification moves and
rests inside a three-wrap envelope. "Three-layer" is literal: **seal** (can't read it), **label**
(can act on it without reading it), **thread** (can prove where it came from). The label is exactly
what `aog-gateway` reads for DSPM-informed routing — the flagship integration rides this wrap.

## Structure
```json
{
  "envelope_id": "env_2026-07-03T18-00-00Z_9d4e",

  "seal": {
    "aead_alg": "AES-256-GCM",
    "data_key_wrapped": "openbao:transit:keys/tenant-baap:v3:...",
    "nonce": "base64...",
    "ciphertext": "base64...",
    "aad_hash": "blake3:..."
  },

  "label": {
    "classification": "restricted",
    "compliance_scopes": ["hipaa"],
    "origin": "svc:aeneas-gateway",
    "permitted_ops": ["unseal_local"],
    "permitted_destinations": ["local_only"],
    "detected_entities": ["phi.mrn", "phi.name"]
  },

  "thread": {
    "created_at": "2026-07-03T18:00:00Z",
    "authorizing_token_id": "tok_...",
    "previous_hash": "blake3:...",
    "signatures": [
      { "alg": "ml-dsa-87", "key_id": "seal-2026-q3", "value": "base64..." }
    ]
  }
}
```

## The three wraps
| Wrap | Owns | Reuse | Read without unseal? |
|---|---|---|---|
| **seal** | AEAD ciphertext + OpenBao-transit-wrapped per-envelope data key | `mai-vault` `AeadSealer` + OpenBao transit | No — needs a token + boundary. |
| **label** | classification, scopes, permitted ops/destinations, detected entities | mai-compliance classifiers (`phi`/`itar`/`ocap`/`tech_data`) | **Yes** — machine-readable; policy acts on it un-sealed. |
| **thread** | authorizing `token_id`, BLAKE3 chain link, signatures | `fabric-proof` | Yes (metadata only). |

## Invariants
- Unseal requires a trust token whose scope permits `label.permitted_ops` and is inside `label.permitted_destinations`; every unseal/reseal emits a receipt (`envelope_id` set).
- The label is produced from the payload at seal time and signed inside the thread — it cannot be altered without breaking the signature.
- Tampering any wrap ⇒ verification fails with a wrap-specific reason (`seal.aead`, `label.signature`, `thread.chain`).

## Verify gate (0.6)
A test that reads `label` (classification + destinations) **without** unsealing the payload; a
seal→unseal round-trip against live OpenBao transit (deferred to F4's live gate, stubbed here);
a tamper test per wrap.
