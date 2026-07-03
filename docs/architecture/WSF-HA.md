# WSF HA & Hardening (SOV-W10)

How the WSF trust plane runs in production: high availability, horizontal scale,
zero-downtime key rotation, and the guard that stops a dev fixture reaching prod.
Reference topology: `deployment/wsf-ha/docker-compose.yml`.

## 1. Topology

```
            ┌──────────── load balancer ────────────┐
            │                                        │
      wsf-api / wsf-bridge (N replicas, stateless)   │
            │            │             │             │
            ▼            ▼             ▼             ▼
     ┌─────────── OpenBao Raft cluster (>=3, auto-unseal) ───────────┐
     │   trust core: AppRole, KV (tenants/creds/revocations), Transit │
     └────────────────────────────────────────────────────────────────┘
      Postgres (service state)        MinIO / S3 (sealed envelopes, evidence)
```

- **OpenBao** runs HA on **integrated Raft storage** with **auto-unseal** (a
  transit/KMS seal — never manual unseal, never dev mode). Run **≥3 nodes** with
  `retry_join` so a node loss keeps a quorum. `deployment/wsf-ha/` shows one node
  + the `BAO_LOCAL_CONFIG` shape; production adds the peer join stanzas.
- **Postgres** holds service state; **MinIO/S3** holds sealed envelopes + evidence
  packs. Both are the durable stores behind the stateless services.

## 2. Horizontal scale (bridge & services are stateless)

Every WSF service does its **own AppRole login per call** and holds no shared
session state (verified in W1–W9: `TrustBridge`, `AwsStsBroker`, `SealService`,
`GcpBroker`, `AzureBroker`, `TenantAdmin` are all constructed per request-handler
and carry only config + an OpenBao client). So they scale by adding replicas
behind a load balancer — no sticky sessions, no leader election. The only
stateful component is OpenBao itself (Raft) + the two data stores.

## 3. Zero-downtime signing-key rotation

Signing-anchor rotation uses `wsf_hardening::KeyRing` — a set of accepted public
keys. Verifiers hold the ring; issuers sign with the **current** key.

Ceremony:

1. **Add** the new key: `ring.rotate_in(new_pub)`. The ring now accepts **both**
   the old and new keys — every in-flight token still verifies.
2. **Migrate** issuers (bridge replicas) to sign with the new key. Roll them one
   at a time; tokens from old and new replicas both verify against the ring.
3. **Drain** the old key's max token TTL (e.g. 15 min), then `ring.retire_oldest()`
   on the verifiers. Only the new key is accepted thereafter.

No window where a validly-issued token is refused → **zero downtime**. Proven by
`wsf-hardening`'s `key_rotation_is_zero_downtime` test (old- and new-key tokens
both verify mid-rotation; the old key fails only after retire).

## 4. Production guard (closes the "proven only against dev" debt)

`wsf_hardening::production_guard` / `assert_production_ready` fail closed on dev
fixtures when `mode = Production`:

| Code | Rejects |
|---|---|
| `insecure_transport` | a non-`https://` OpenBao address (plaintext) |
| `dev_root_token` | the OpenBao dev `root` token (or empty) |
| `weak_hmac_key` | a subject-HMAC key < 32 bytes |
| `dev_hmac_key` | a uniform-byte HMAC key (the `vec![7u8; 32]`-style test fixtures) |

Wire `assert_production_ready` into each service's startup (a `PROD-*`-style
guard, mirroring the MAI `production_guard` pattern) so a service **refuses to
boot** against dev OpenBao / dev keys in production. The guard is a no-op in
`DeployMode::Dev`, so local dev + the live tests are unaffected.

## 5. Open debt (honest)

- The live-service gates (W1–W9) run against **dev-mode** OpenBao (Moto/mock for
  the clouds). Production hardening — Raft HA, auto-unseal, TLS, real cloud STS —
  is exercised by the **D-phase** (`D5` live-backend + live-OpenBao-HA suite, `D7`
  burn-in). W10 provides the *mechanisms* (KeyRing, guard, HA compose) that those
  D-phase gates prove end-to-end on target hardware.
- The HA compose references `islandmountain/wsf-api:latest` — that image is built
  by the D-phase appliance build (`D1`/`D3`), not yet in-tree.
