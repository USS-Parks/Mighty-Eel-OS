# Trust Manifold Specification (BF-1)

**Status:** Backfill spec — landed during Session 39 mainline work
**Equivalent V2 Session:** 26b (OpenBao Trust Manifold spec)
**Audience:** Build engineers, compliance engineers, acquirers performing technical due diligence
**Companion docs:** [OPENBAO-INTEGRATION.md](./OPENBAO-INTEGRATION.md), [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md), forthcoming `TRUST-BUNDLE-SPEC.md` (BF-3), `LOCAL-TRUST-CACHE.md` (BF-4), `AUDIT-CORRELATION.md` (BF-5).

---

## 1. Purpose

The Trust Manifold is the **enterprise trust fabric** that connects cloud-scale identity, credential validation, PKI, signing, and revocation to the **local-first** MAI + Lamprey appliance. It exists so customers in regulated industries (healthcare, defense, tribal government) can prove identity, policy, and route decisions to auditors without ever shipping regulated payloads off the appliance.

The manifold is **not** a replacement for MAI security (API auth, rate limiting, vault encryption) and it is **not** a replacement for Lamprey compliance governance (HIPAA / ITAR-EAR / OCAP classification + policy). It sits between them and provides the trust substrate both depend on.

Stated as a single sentence:

> The cloud trust core validates **authority**. The local node protects **data**. Lamprey decides whether the request is **allowed**.

---

## 2. Three Rings

The manifold is organised into three rings. Each ring has a distinct deployment location, lifetime, and threat profile.

| Ring | Name | Primary purpose | Deployment location | Lifetime of state |
|---|---|---|---|---|
| Ring 1 | **OpenBao Trust Core** | Identity, secrets, PKI, Transit signing, token leases, audit devices | Cloud or enterprise cluster | Persistent, multi-year |
| Ring 2 | **Lamprey Trust Bridge** | Converts OpenBao identity events into Lamprey-shaped claims; signs policy bundles | Cloud, paired 1:1 with OpenBao | Persistent, but stateless between calls |
| Ring 3 | **Local Trust Cache** | Offline-verifiable claims, signed policy bundles, revocation snapshots | On every MAI appliance | Bounded by bundle TTL (hours to days) |

### 2.1 Boundary diagram (text)

```text
+----------------------------------------------------------+
|                  Enterprise IdP / SSO                    |
|         (OIDC, SAML, Kerberos, workload identity)        |
+-----------------------------+----------------------------+
                              |
                              v
+----------------------------------------------------------+
|     Ring 1: Cloud OpenBao Trust Core                     |
|     - kv/                  secret store                  |
|     - transit/             signing + verify              |
|     - pki/                 short-lived certs             |
|     - auth/kubernetes/     workload identity             |
|     - auth/userpass/oidc/  human identity                |
|     - audit/file|syslog    tamper-evident events         |
+-----------------------------+----------------------------+
            issues:           |
            - short-lived tokens (TTL minutes)
            - signed PKI leaf certs (TTL hours)
            - Transit-signed policy bundles (TTL hours-days)
            - revocation snapshots (refreshed periodically)
                              |
                              v
+----------------------------------------------------------+
|     Ring 2: Lamprey Trust Bridge                         |
|     - claim issuer (OpenBao identity -> Lamprey claim)   |
|     - policy bundle signer                               |
|     - revocation snapshot signer                         |
|     - audit correlation publisher (metadata-only)        |
+-----------------------------+----------------------------+
                              |
                              v
+----------------------------------------------------------+
|     Ring 3: Local Trust Cache (on each appliance)        |
|     - verified signed claims                             |
|     - verified signed policy bundles                     |
|     - revocation snapshot (with age)                     |
|     - connectivity-state machine                         |
+-----------------------------+----------------------------+
                              |
                              v
+----------------------------------------------------------+
|     Local MAI + Lamprey                                  |
|     - Lamprey Router classifies the request              |
|     - Lamprey Policy applies HIPAA / ITAR / OCAP +       |
|       TrustContext from the local cache                  |
|     - MAI Scheduler places the inference locally         |
|     - Audit log records credential event + decision      |
+----------------------------------------------------------+
                              |
                              | metadata-only sync, never payloads
                              v
+----------------------------------------------------------+
|     Cloud Audit Correlation Layer (BF-5)                 |
+----------------------------------------------------------+
```

### 2.2 Clean system boundary

The Trust Manifold **may** move:

- Identity metadata (subject id, tenant id, roles, scopes)
- Short-lived claims (`claim_id`, expiry, allowed routes)
- Signatures (over policy bundles and claims)
- mTLS certificates (PKI-issued, short-lived)
- Revocation snapshots (which claims/keys/bundles are no longer valid)
- Signed policy bundles (the rules the policy runtime applies)
- Audit correlation IDs (`credential_event_id`, `lamprey_decision_id`, `mai_request_id`)
- Tenant policy metadata (which compliance scopes a tenant has bought)

The Trust Manifold **must not** move by default:

- Prompts
- Completions
- Embeddings
- PHI content
- ITAR/EAR-controlled technical content
- OCAP-governed tribal data
- Any regulated payload, in any direction

This boundary is the product. Customers buy Island Mountain because the appliance never has to leak the protected data to the cloud trust system in order to be trustworthy.

---

## 3. Tenant Model

A **tenant** is the unit of policy authority. A tenant could be:

- A single hospital system (HIPAA)
- A defense contractor's program office (ITAR/EAR)
- A tribal nation or specific tribal program (OCAP)
- A hybrid (e.g., a tribal health authority — OCAP + HIPAA)

### 3.1 Tenant attributes

| Attribute | Description |
|---|---|
| `tenant_id` | Stable identifier, lowercase kebab-case (e.g. `bay-area-pediatrics`) |
| `display_name` | Human-readable label |
| `compliance_scopes` | Subset of `{ "hipaa", "itar_ear", "ocap" }` the tenant has licensed |
| `default_allowed_routes` | Subset of `{ "local_only", "local_preferred", "cloud_allowed" }` |
| `data_classifications` | Allowed `max_data_classification` ceiling (e.g. `"restricted"`, `"controlled"`) |
| `governance_metadata` | Free-form, tenant-specific (treaty references for OCAP tenants, BAA reference numbers for HIPAA, etc.) |

### 3.2 Tenant lifecycle

1. **Provisioning** — Sales/Compliance signs the contract; tenant attributes are written into the OpenBao Trust Core under `kv/tenants/<tenant_id>`.
2. **Issuance** — The Lamprey Trust Bridge picks up the tenant attributes when minting a claim for a subject in that tenant.
3. **Deprovisioning** — Tenant attributes are revoked; an updated revocation snapshot propagates to every appliance.

A subject belongs to exactly one tenant for the lifetime of a claim. Cross-tenant access requires a new claim (and a fresh trust evaluation).

---

## 4. Lamprey Claim Schema

A **claim** is the short-lived assertion the Lamprey Trust Bridge produces from an OpenBao identity event. Claims are the canonical input to every Lamprey policy decision.

### 4.1 Wire shape

```json
{
  "claim_id": "clm_2026-05-22T18-00-00Z_a7f3",
  "issued_at": "2026-05-22T18:00:00Z",
  "expires_at": "2026-05-22T18:15:00Z",
  "issuer": "lamprey-trust-bridge",
  "trust_bundle_version": "2026.05.22.001",

  "tenant_id": "bay-area-pediatrics",
  "subject_id": "user:alice@bayarea-peds.example",
  "subject_hash": "hmac:7c2f3a...",
  "service_identity": null,

  "roles": ["clinician", "supervising-pharmacist"],
  "compliance_scopes": ["hipaa"],
  "allowed_routes": ["local_only"],
  "allowed_models": ["llama-3-8b-instruct", "mistral-7b-q4"],
  "max_data_classification": "restricted",

  "country": "US",
  "person_type": "us_person",

  "offline_mode": false,
  "revocation_status": "unknown",

  "signature": {
    "alg": "ed25519",
    "key_id": "lamprey-bridge-2026-q2",
    "value": "base64..."
  }
}
```

### 4.2 Field semantics

| Field | Required | Purpose |
|---|---|---|
| `claim_id` | yes | Globally unique. Audit logs reference this id. |
| `issued_at` / `expires_at` | yes | UTC ISO-8601. Window is short (5-15 minutes typical). |
| `issuer` | yes | Always the Lamprey Trust Bridge service identity. |
| `trust_bundle_version` | yes | Version of the policy bundle this claim was issued against. |
| `tenant_id` | yes | See §3. |
| `subject_id` | yes | Human-readable identity (logged with care). |
| `subject_hash` | yes | HMAC of `subject_id` with a per-tenant key. The audit log uses **this**, not `subject_id`, for compliance with HIPAA min-necessary. |
| `service_identity` | yes for service-to-service claims, null otherwise | One of the nine identities in [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md). |
| `roles` | yes | Application-defined RBAC roles. |
| `compliance_scopes` | yes | Subset of `{ "hipaa", "itar_ear", "ocap" }`. Determines which Lamprey engines may even apply. |
| `allowed_routes` | yes | Hard ceiling on routing. `local_only` blocks any cloud route at the policy layer regardless of classification. |
| `allowed_models` | optional | Tenant- or subject-specific model allowlist. |
| `max_data_classification` | yes | Ceiling on classification the subject may handle. |
| `country` / `person_type` | yes when ITAR scope present | Feeds the jurisdiction module today (Session 39 `ActorContext`). |
| `offline_mode` | yes | True when the appliance is in degraded / air-gap mode. |
| `revocation_status` | yes | `valid` / `revoked` / `stale` / `unknown`. |
| `signature` | yes | Ed25519 over the canonical JSON of the rest of the claim. |

### 4.3 Subject hashing rule

Subject identifiers must never appear in audit logs in raw form. The bridge computes:

```text
subject_hash = HMAC-SHA256(per_tenant_key, subject_id)
```

The per-tenant HMAC key lives in OpenBao at `kv/tenants/<tenant_id>/subject-hmac` and is rotated on a 90-day cadence. The current `key_id` is part of the hash material so the audit layer can join across rotations.

---

## 5. Offline Trust Model

The appliance must continue to make safe decisions when the cloud is unreachable. This is a hard requirement: tribal sites with intermittent satellite links, defense customers in air-gapped facilities, and rural health clinics during outages all need the same answer — **the appliance keeps working**, but its **safety surface shrinks**.

### 5.1 Connectivity states

| State | Definition | Effect on policy |
|---|---|---|
| **Connected** | Cloud Bridge reachable, bundle and revocation snapshot are fresh | Full policy surface. Live claim verification allowed. |
| **Degraded** | Cloud unreachable but cached bundle + revocation snapshot are within TTL | Continue with cached material. Mark all audit events `trust_mode=degraded`. |
| **Stale** | Bundle is past `soft_expiry` but before `hard_expiry` | Continue, but emit warnings; deny issuance of new long-lived sessions. |
| **Expired** | Bundle past `hard_expiry` | Restrict to local admin / emergency mode. Cloud routes blocked unconditionally. |
| **Air-gapped** | Operator-asserted; no network attempted | Local-only inference. Cloud routes are not even attempted. |

### 5.2 Bundle TTLs

| Bundle kind | Soft TTL | Hard TTL |
|---|---|---|
| Policy bundle | 6 hours | 72 hours |
| Revocation snapshot | 1 hour | 24 hours |
| Tenant attribute snapshot | 6 hours | 7 days |

These are defaults. Tenants on long-duration air-gap (e.g., shipboard, remote tribal sites) may negotiate longer hard TTLs at provisioning time — never via runtime config.

### 5.3 Safe degradation rules

1. **Default to the most restrictive answer.** When trust material is missing or stale, the policy runtime should treat the claim as if it had narrower scopes, not broader.
2. **Air-gapped mode is sticky.** It can only be cleared by an operator action with the audit reason logged.
3. **No write-side compromises.** Compliance audits, decisions, and reports continue to be written locally; they are queued for metadata-only sync when connectivity returns.

---

## 6. Revocation Model

Revocation is the answer to "this claim / key / bundle is no longer trustworthy". It must work offline.

### 6.1 Revocation snapshot

```json
{
  "snapshot_id": "rev_2026-05-22T18-00-00Z",
  "issued_at": "2026-05-22T18:00:00Z",
  "expires_at": "2026-05-22T19:00:00Z",
  "revoked_claims": ["clm_..."],
  "revoked_subjects": ["user:bob@..."],
  "revoked_service_identities": [],
  "revoked_bundle_versions": ["2026.05.20.003"],
  "revoked_signing_keys": [],
  "signature": { "alg": "ed25519", "key_id": "...", "value": "base64..." }
}
```

### 6.2 Resolution order

When the policy runtime checks a claim:

1. Verify signature against a trusted bridge signing key.
2. Verify the claim has not expired.
3. Verify the claim, subject, bundle version, and signing key are **not** in the current revocation snapshot.
4. Verify the snapshot itself has not expired (per §5.1 stale/expired rules).

Any failure produces a `DenyExport` outcome with a `revocation.*` matched-rule tag and a structured reason. Audit always records which check failed.

### 6.3 Emergency revocation

The Lamprey Trust Bridge supports an `emergency_revoke` endpoint that produces a short-TTL snapshot regardless of normal cadence. Appliances apply it on next poll. In air-gap mode, an operator can install an emergency revocation snapshot from a removable medium; the snapshot is signed by the same bridge keys.

---

## 7. Threat Model

This is not a full STRIDE document. It is the set of threats that drive Trust Manifold design choices.

### 7.1 In scope

| Threat | Mitigation |
|---|---|
| Stolen long-lived credentials | All claims are short-lived (5-15 min). No long-lived bearer tokens cross the boundary. |
| Cloud trust compromise leaks customer data | Cloud trust system never sees prompts, completions, or regulated payloads. |
| Network partition causes outage | Local Trust Cache + signed bundles + offline mode. |
| Bundle tampering in transit | Ed25519 signatures over canonical bundle bytes. |
| Bundle replay across tenants | `tenant_id` and `trust_bundle_version` bound into the signature payload. |
| Insider with raw subject IDs misuses audit logs | Subject IDs are HMAC'd before audit (§4.3). |
| Wrong service speaks for another service | Service identities are mutually exclusive and OpenBao policies are per-service (see [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md)). |
| Stale revocation lets a revoked claim work | Snapshots have a hard TTL; expired snapshots collapse the appliance to local-admin mode. |
| Operator forgets the appliance is air-gapped and routes cloud | Air-gap mode is sticky and cloud routes are denied at the policy layer (not just unreachable). |

### 7.2 Out of scope (deferred or owned elsewhere)

- Physical tamper of the appliance — addressed by Session 28 air-gap hardware enforcement, not by the manifold.
- Side-channel attacks on local inference — out of scope.
- Cryptographic primitives themselves (Ed25519, HMAC-SHA256 are assumed sound). The manifold inherits OpenBao's crypto.
- Network-level DDoS against the cloud trust core — OpenBao operational concern.

---

## 8. Responsibility Map

This is the single most important table in this document. It tells engineers and acquirers where each capability lives.

### 8.1 OpenBao (Ring 1) owns

- Identity authentication (OIDC, SAML, Kubernetes service-account, AppRole, userpass)
- Secret storage (`kv/`)
- PKI issuance (`pki/` — short-lived leaf certs for mTLS)
- Transit signing/verification (`transit/`)
- Token leasing and renewal
- Token revocation
- Audit devices (file, syslog, socket — tamper-evident)
- Policy-controlled access to all of the above

### 8.2 Lamprey Trust Bridge (Ring 2) owns

- Conversion of OpenBao identity events into Lamprey claims
- Tenant attribute lookup and binding into claims
- Subject HMAC computation
- Policy bundle composition and signing
- Revocation snapshot composition and signing
- Audit correlation publication (metadata-only) to the cloud audit layer

### 8.3 Local Trust Cache + MAI / Lamprey Local (Ring 3) own

- Local verification of claims, bundles, and snapshots (no cloud round-trip on the request path)
- Connectivity state machine (§5.1)
- Local-only routing enforcement
- Lamprey compliance classification (HIPAA / ITAR-EAR / OCAP)
- Lamprey policy decisions
- MAI scheduler placement (which model, which GPU, batch slot)
- Local tamper-evident audit log
- Offline audit queue
- Local inference

### 8.4 Nothing in the manifold owns

- Prompts
- Completions
- Embeddings
- Model weights
- Customer regulated payloads of any kind

This list is the negation of §2.2. It is repeated here because acquirers ask twice.

---

## 9. Relationship to TrustContext (BF-2)

The Trust Manifold produces a verified claim. The policy runtime consumes a [`TrustContext`](./SERVICE-IDENTITY.md#trustcontext-shape) — a flat decision-time projection of that claim. The two are not the same thing:

- A **claim** is the signed artefact crossing the network. It is canonical, audit-grade, and refers back to OpenBao identity events.
- A **TrustContext** is the in-memory struct the policy runtime evaluates against. It is constructed from a verified claim plus the local connectivity state.

The mapping is one-to-one for fields that exist in both. The `offline_mode` and `revocation_status` fields are filled in from the local trust cache at evaluation time, not from the claim itself, because they reflect appliance state, not subject state. See [SERVICE-IDENTITY.md §4](./SERVICE-IDENTITY.md) for the field-by-field projection.

---

## 10. What this document does NOT cover

| Topic | Where it lives |
|---|---|
| Specific OpenBao mount layout and policy paths | [OPENBAO-INTEGRATION.md](./OPENBAO-INTEGRATION.md) |
| Service identity catalog and per-service policies | [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md) |
| Signed bundle wire format and verification algorithm | `TRUST-BUNDLE-SPEC.md` (BF-3, before S41 closes) |
| Local trust cache schema and state-machine code | `LOCAL-TRUST-CACHE.md` (BF-4, before S42 starts) |
| Audit correlation event schema | `AUDIT-CORRELATION.md` (BF-5, during S42) |
| Cluster topology, HA, disaster recovery for OpenBao | OpenBao operations runbook (not in this repo) |

---

## 11. Acceptance Criteria for BF-1

- [x] Trust Manifold architecture is documented (this file).
- [x] OpenBao is clearly assigned to trust, secrets, PKI, signing, revocation and audit-device functions (§8.1).
- [x] Lamprey remains responsible for compliance classification and policy decisions (§8.2-8.3).
- [x] MAI remains responsible for local inference and hardware-aware scheduling (§8.3).
- [x] Claim schema is defined and Session 39 can begin consuming it via the TrustContext projection (§4, §9).
- [x] No part of the architecture requires regulated payloads to leave the local node (§2.2, §8.4).
