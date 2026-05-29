# OpenBao Integration (BF-1)

**Status:** Backfill spec — landed during Session 39 mainline work
**Equivalent V2 Session:** 26b (OpenBao Trust Manifold spec)
**Audience:** Build engineers configuring the trust core, operators provisioning tenants, acquirers evaluating the trust substrate
**Companion docs:** [TRUST-MANIFOLD.md](./TRUST-MANIFOLD.md), [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md)

---

## 1. Why OpenBao

[OpenBao](https://openbao.org/) is the community fork of HashiCorp Vault. We adopt it instead of building our own credential store for three reasons:

1. **Mature primitives.** Token leasing, PKI, Transit signing, dynamic secrets, audit devices, response wrapping, and policy ACLs already exist, hardened by ten years of production use as Vault.
2. **Open licence.** OpenBao is MPL-2.0 and community-governed, removing the BUSL licence risk Island Mountain inherits if we built against upstream Vault.
3. **API compatibility.** Vault clients work against OpenBao without code changes, so any acquirer with existing Vault tooling integrates with zero friction. This is part of the [acquisition narrative](#7-buyer-narrative).

We do **not** adopt OpenBao to replace MAI auth (Session 26 API keys) or Lamprey policy. OpenBao is the **substrate**; MAI and Lamprey are the products that consume it.

---

## 2. Mount Layout

The Ring-1 OpenBao Trust Core uses six logical mounts. Customers may run additional mounts for their own purposes; the manifold only requires these.

| Mount | Type | Purpose | Sealed? |
|---|---|---|---|
| `kv/` | KV v2 | Tenant attributes, per-tenant HMAC keys, service-identity metadata | Yes |
| `transit/` | Transit | Signs claims, policy bundles, revocation snapshots. Never exposes private keys. | Yes |
| `pki/` | PKI | Issues short-lived leaf certs for mTLS between MAI / Lamprey / appliance | Yes |
| `auth/kubernetes/` | Kubernetes auth | Authenticates `mai-*` and `lamprey-*` cloud workloads | n/a |
| `auth/approle/` | AppRole | Authenticates appliances (Local Trust Cache) | n/a |
| `auth/oidc/` | OIDC | Authenticates human operators for tenant provisioning | n/a |
| `audit/file/` | File audit device | Tamper-evident audit stream (forwarded to SIEM) | n/a |

### 2.1 Tenant data under `kv/`

```text
kv/
  tenants/
    <tenant_id>/
      attributes              # tenant model fields (see TRUST-MANIFOLD.md §3.1)
      subject-hmac            # current 32-byte HMAC key, with key_id
      subject-hmac-archive    # rotated keys, retained for audit lookup
      governance              # OCAP treaty refs, BAA reference numbers, ITAR profile id
  services/
    <service_identity>/
      claim-template          # default scopes for service-to-service claims
```

### 2.2 Transit keys

| Key name | Purpose | Algorithm | Rotation |
|---|---|---|---|
| `lamprey-claim-signer` | Signs Lamprey claims (§4.1 of TRUST-MANIFOLD.md) | Ed25519 | Quarterly |
| `lamprey-bundle-signer` | Signs policy bundles | Ed25519 | Quarterly |
| `lamprey-revocation-signer` | Signs revocation snapshots | Ed25519 | Monthly (faster, smaller blast radius if compromised) |

Private keys never leave OpenBao. Bridges call `transit/sign/<key_name>` and receive the signature. Appliances call `transit/verify/<key_name>` only when bootstrapping — for the request path they verify locally using cached public-key material distributed in the trust bundle.

### 2.3 PKI roles

| Role | Subject CN pattern | TTL | Purpose |
|---|---|---|---|
| `lamprey-bridge` | `*.lamprey-bridge.local` | 12 h | mTLS between bridge instances and OpenBao |
| `mai-appliance` | `appliance-*.mai.local` | 24 h | Appliance authenticates upstream |
| `lamprey-service` | `lamprey-<service>.local` | 6 h | Inter-service mTLS in the cloud Lamprey cluster |

All certificates carry the service identity in the SAN URI (`spiffe://island-mountain/<service_identity>`) so policy engines can authenticate the caller without trusting cleartext headers.

---

## 3. Auth Methods

### 3.1 Cloud workloads (Kubernetes)

Lamprey Trust Bridge instances run in Kubernetes. They authenticate via `auth/kubernetes/` using their service-account JWT. The token's `kubernetes.io/serviceaccount/service-account.name` claim is bound 1:1 to a service identity (see [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md)).

```hcl
# Pseudocode: provisioning an identity
vault write auth/kubernetes/role/lamprey-router \
  bound_service_account_names=lamprey-router \
  bound_service_account_namespaces=lamprey-prod \
  policies=lamprey-router-policy \
  ttl=15m
```

### 3.2 Appliances (AppRole)

A MAI appliance does not run in Kubernetes and cannot present a service-account JWT. It authenticates via `auth/approle/`:

- **role_id** is provisioned per-appliance at manufacture/install.
- **secret_id** is wrapped in a short-lived response-wrapping token delivered out-of-band (signed configuration packet).
- The appliance unwraps once, exchanges role_id + secret_id for a short-lived OpenBao token, then immediately drops to its local trust cache.

The local trust cache then pulls the signed policy bundle and revocation snapshot. The appliance does **not** hold a long-lived OpenBao token; it re-authenticates per refresh cycle.

### 3.3 Humans (OIDC)

Human operators (tenant provisioning, emergency revocation, manual bundle rotation) authenticate via OIDC against the customer's existing IdP. No human reads or writes appliance data through OpenBao — they only manipulate tenant configuration and trust artefacts.

---

## 4. Claim Issuance Flow

This is the canonical sequence for a subject obtaining a Lamprey claim.

```text
Subject (human or workload)
       |
       | 1. Authenticate to OpenBao (auth/<method>)
       v
Cloud OpenBao Trust Core
       |
       | 2. Identity event published to audit device
       | 3. Returns short-lived OpenBao token to Lamprey Trust Bridge
       v
Lamprey Trust Bridge
       |
       | 4. Look up tenant attributes (kv/tenants/<tenant_id>/attributes)
       | 5. Look up subject HMAC key (kv/tenants/<tenant_id>/subject-hmac)
       | 6. Compose claim JSON (TRUST-MANIFOLD.md §4)
       | 7. Call transit/sign/lamprey-claim-signer
       | 8. Return signed claim to caller
       v
Caller (with signed claim)
       |
       | 9. Present claim to local MAI appliance over mTLS
       v
Local Trust Cache
       |
       | 10. Verify claim signature against cached bridge public key
       | 11. Verify expiry, revocation, bundle version
       | 12. Project into TrustContext (§5)
       v
Lamprey Policy Runtime
       |
       | 13. Evaluate HIPAA + ITAR/EAR + OCAP against TrustContext
       v
MAI Scheduler -> Inference -> Audit
```

Steps 4-8 happen in the bridge. Steps 10-13 happen on the appliance. **No part of the request payload (prompt, completion, embedding) flows through steps 1-8.**

---

## 5. TrustContext Projection

The local policy runtime consumes a flat `TrustContext` struct. It is constructed in step 10-12 above from:

1. The verified Lamprey claim (steps 1-8).
2. The local connectivity state machine ([TRUST-MANIFOLD.md §5](./TRUST-MANIFOLD.md#5-offline-trust-model)).
3. The current revocation snapshot.

Field-by-field:

| TrustContext field | Source |
|---|---|
| `tenant_id` | claim |
| `subject_id` | claim (only retained in-process; never logged) |
| `subject_hash` | claim |
| `roles` | claim |
| `compliance_scopes` | claim |
| `allowed_routes` | claim |
| `allowed_models` | claim |
| `max_data_classification` | claim |
| `service_identity` | claim (null for human subjects) |
| `trust_bundle_version` | claim |
| `claim_id` | claim |
| `offline_mode` | **local state machine** — true when appliance is degraded/expired/air-gapped |
| `revocation_status` | **local revocation snapshot** |

The implementation lives in `mai-compliance::trust` (BF-2). See [SERVICE-IDENTITY.md §4](./SERVICE-IDENTITY.md) for the exact Rust type.

---

## 6. Local Operation Without Online OpenBao

The system must function with no cloud OpenBao reachable. This is enabled by:

| Capability | Local artefact |
|---|---|
| Verify Lamprey claim signatures | Cached `lamprey-claim-signer` public key (rotated quarterly, distributed in the trust bundle) |
| Verify policy bundle signatures | Cached `lamprey-bundle-signer` public key |
| Verify revocation snapshot signatures | Cached `lamprey-revocation-signer` public key |
| Issue local mTLS leaves | Locally-generated intermediate CA signed once by OpenBao at provisioning; appliance issues short leaves under it |
| Local-only token leases | Not required — local services trust each other via mTLS bound to the SAN URI |

The intermediate CA is bound to a single appliance serial number and a single tenant set, so a compromised appliance cannot impersonate another.

### 6.1 Hardware air-gap mode

When the air-gap switch is engaged (Session 28 work), the appliance:

1. Forbids any outbound connection to OpenBao or the bridge.
2. Treats every claim as if signed by the cached public key only.
3. Refuses to refresh the policy bundle or revocation snapshot.
4. Records every decision with `trust_mode=air_gapped`.
5. Collapses to local-admin emergency mode when bundles or snapshots hit hard expiry.

---

## 7. Buyer Narrative

For Session 45 acquisition documentation, the OpenBao integration story compresses to:

1. **Trust is not a new invention.** OpenBao is a fork of HashiCorp Vault. Acquirers with existing Vault deployments integrate by pointing at a different host.
2. **Trust never gets the data.** OpenBao validates authority. The appliance protects the payload. Lamprey decides whether the request is allowed. These are three separate concerns, each in a separate trust ring.
3. **Trust survives disconnection.** Every appliance carries signed bundles and a local revocation snapshot. The cloud trust core can be unreachable for days without the appliance becoming unsafe — only its safety surface shrinks.
4. **Trust is auditable end-to-end.** Every credential event in OpenBao's audit device correlates to a Lamprey decision and to a MAI inference event via the BF-5 audit correlation IDs.

The acquirer is buying a system in which the trust substrate is well-understood open-source software, and the differentiating IP is the layer of compliance governance that sits on top.

---

## 8. What this document does NOT cover

| Topic | Where it lives |
|---|---|
| Architecture rationale and threat model | [TRUST-MANIFOLD.md](./TRUST-MANIFOLD.md) |
| Service identity catalog and policies | [SERVICE-IDENTITY.md](./SERVICE-IDENTITY.md) |
| Signed bundle wire format | `TRUST-BUNDLE-SPEC.md` (BF-3, before S41 closes) |
| Local trust cache schema and state machine code | `LOCAL-TRUST-CACHE.md` (BF-4, before S42 starts) |
| Audit correlation event schema | `AUDIT-CORRELATION.md` (BF-5, during S42) |
| OpenBao HA cluster topology, disaster recovery | OpenBao operations runbook (not in this repo) |

---

## 9. Acceptance Criteria for BF-1 (this file's share)

- [x] OpenBao is clearly assigned to identity / secrets / PKI / signing / revocation / audit-device functions (§2, §8).
- [x] Cloud workloads, appliances, and humans each have a documented auth path (§3).
- [x] The claim issuance flow is explicit about where signing happens and where regulated payloads do **not** go (§4).
- [x] Local operation without online OpenBao is enumerated, including air-gap (§6).
- [x] An acquirer-facing narrative exists for the Session 45 package (§7).
