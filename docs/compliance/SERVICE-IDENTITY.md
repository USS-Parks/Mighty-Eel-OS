# Service Identity and Claims Map (BF-2)

**Status:** Backfill spec — landed during Session 39 mainline work
**Equivalent V2 Session:** 26d (Service identity model and OpenBao policy map)
**Audience:** Build engineers wiring services, security reviewers verifying least-privilege, Session 41 policy runtime author
**Companion docs:** [TRUST-MANIFOLD.md](./TRUST-MANIFOLD.md), [OPENBAO-INTEGRATION.md](./OPENBAO-INTEGRATION.md)

---

## 1. Purpose

Every service that participates in the Trust Manifold needs a **distinct, narrow identity**. This document is the catalog:

- The nine identities the architecture requires.
- The OpenBao policy path convention each identity uses.
- The TrustContext fields each identity may legitimately populate.
- The denied-access test plan that proves the boundaries hold.

The principle: **no service shares a token with any other service.** A compromise of `lamprey-router` must not yield `lamprey-policy`'s capabilities. The policy engine can later distinguish user claims from service claims because the `service_identity` field on a `TrustContext` is non-null only for the service path.

---

## 2. Service Identity Catalog

| # | Identity | Ring | Responsibility | Auth method | Token TTL |
|---:|---|---|---|---|---|
| 1 | `mai-api` | Local (R3) | HTTP/gRPC ingress on the appliance | AppRole | 1 h |
| 2 | `mai-scheduler` | Local (R3) | Inference placement | AppRole | 1 h |
| 3 | `mai-adapter-manager` | Local (R3) | Backend adapter lifecycle | AppRole | 1 h |
| 4 | `lamprey-router` | Local (R3) | Request classification + sensitivity scoring | AppRole | 1 h |
| 5 | `lamprey-policy` | Local (R3) | HIPAA / ITAR-EAR / OCAP policy evaluation | AppRole | 1 h |
| 6 | `lamprey-audit` | Local (R3) | Tamper-evident audit log writer | AppRole | 1 h |
| 7 | `lamprey-dashboard` | Local (R3) | Read-only dashboard backend | AppRole | 30 m |
| 8 | `local-trust-cache` | Local (R3) | Bundle + revocation snapshot fetcher | AppRole | 15 m |
| 9 | `audit-correlation-service` | Cloud (R2) | Metadata-only audit sync upstream | Kubernetes auth | 15 m |

Identities 1-8 live on every appliance. Identity 9 lives in the cloud Lamprey cluster.

### 2.1 Naming rule

All identities are lowercase kebab-case, prefixed by the product they belong to (`mai-*`, `lamprey-*`, or `audit-*`). The identity string appears verbatim in:

- The OpenBao policy path.
- The mTLS SAN URI: `spiffe://island-mountain/<service_identity>`.
- The TrustContext `service_identity` field.
- The Lamprey audit log `service_identity` field.

A typo on any of these three surfaces is detected at startup: the service refuses to start if its presented identity does not match its configured policy.

---

## 3. OpenBao Policy Path Convention

The convention follows the same shape as upstream Vault best practice:

```text
auth/<method>/role/<service_identity>
sys/policies/acl/<service_identity>
kv/data/services/<service_identity>/*
transit/<sign|verify>/<key_name>  # restricted per service
pki/issue/<role>                  # restricted per service
```

Every service identity has its own ACL policy under `sys/policies/acl/<service_identity>`. The policy file lives in version control under `config/openbao/policies/<service_identity>.hcl` (to be created during BF-3 implementation work; this BF-2 spec defines the shape only).

### 3.1 Per-service policy map

| Service identity | May read | May write | May sign | May verify | May issue cert |
|---|---|---|---|---|---|
| `mai-api` | `kv/services/mai-api/*` | — | — | — | `pki/issue/mai-appliance` |
| `mai-scheduler` | `kv/services/mai-scheduler/*` | — | — | — | `pki/issue/mai-appliance` |
| `mai-adapter-manager` | `kv/services/mai-adapter-manager/*` | — | — | — | `pki/issue/mai-appliance` |
| `lamprey-router` | `kv/services/lamprey-router/*` | — | — | `transit/verify/lamprey-bundle-signer`, `transit/verify/lamprey-claim-signer` | `pki/issue/lamprey-service` |
| `lamprey-policy` | `kv/services/lamprey-policy/*`, `kv/tenants/*/attributes` | — | — | `transit/verify/lamprey-bundle-signer`, `transit/verify/lamprey-revocation-signer` | `pki/issue/lamprey-service` |
| `lamprey-audit` | `kv/services/lamprey-audit/*` | (audit log is local, not in OpenBao) | — | — | `pki/issue/lamprey-service` |
| `lamprey-dashboard` | `kv/services/lamprey-dashboard/*` | — | — | — | `pki/issue/lamprey-service` |
| `local-trust-cache` | `kv/services/local-trust-cache/*` | — | — | `transit/verify/lamprey-bundle-signer`, `transit/verify/lamprey-revocation-signer`, `transit/verify/lamprey-claim-signer` | — |
| `audit-correlation-service` | — | — | — | — | `pki/issue/lamprey-bridge` |

Reading this table is the security review of the manifold. Anything not listed is denied by default — OpenBao's deny-by-default ACL evaluation is one of the reasons we picked it over rolling our own.

### 3.2 Notable narrowings

- **No service signs.** Only the Lamprey Trust Bridge holds `transit/sign/*` capability, and it is not in this catalog because it is not a participant in the per-appliance trust set — it is the issuer that sits above all of them.
- **`lamprey-dashboard` is read-only.** It cannot issue claims, cannot write audit, cannot decrypt secrets. A compromise yields only what the dashboard already renders.
- **`lamprey-audit` writes locally.** The audit log file lives on the appliance under tamper-evident hash chain (Session 42 work). OpenBao is not the audit store.
- **`local-trust-cache` can verify all three signer keys** — claims, bundles, snapshots — because it is the gatekeeper that decides whether anything else even sees a valid TrustContext.

---

## 4. TrustContext Shape

This is the Rust type Session 39 (BF-2) introduces in `mai-compliance::trust`. Every Lamprey-side component from Session 39 onward accepts a `&TrustContext` on its decision path.

### 4.1 Canonical Rust shape (informative)

```rust
pub struct TrustContext {
    pub tenant_id: TenantId,
    pub subject_id: SubjectId,
    pub subject_hash: SubjectHash,
    pub roles: BTreeSet<String>,
    pub compliance_scopes: BTreeSet<ComplianceScope>,
    pub allowed_routes: BTreeSet<AllowedRoute>,
    pub allowed_models: BTreeSet<String>,
    pub max_data_classification: DataClassification,
    pub service_identity: Option<ServiceIdentity>,
    pub trust_bundle_version: String,
    pub claim_id: String,
    pub offline_mode: bool,
    pub revocation_status: RevocationStatus,
}
```

The exact field types ship in `mai-compliance/src/trust.rs`; this spec is intentionally one step ahead of the code so reviewers can cross-check.

### 4.2 Field source map

| TrustContext field | Populated from | Notes |
|---|---|---|
| `tenant_id` | claim.tenant_id | required |
| `subject_id` | claim.subject_id | in-memory only; never logged |
| `subject_hash` | claim.subject_hash | audit-safe form |
| `roles` | claim.roles | application-defined RBAC |
| `compliance_scopes` | claim.compliance_scopes | drives which engines may apply |
| `allowed_routes` | claim.allowed_routes | hard ceiling on routing |
| `allowed_models` | claim.allowed_models | empty set = no restriction |
| `max_data_classification` | claim.max_data_classification | classification ceiling |
| `service_identity` | claim.service_identity | `None` for human subjects |
| `trust_bundle_version` | claim.trust_bundle_version | recorded in audit |
| `claim_id` | claim.claim_id | audit correlation key |
| `offline_mode` | **local state machine**, not the claim | reflects appliance state |
| `revocation_status` | **local revocation snapshot** | reflects appliance state |

### 4.3 Compliance scope semantics

```text
hipaa     -> HIPAA engine MAY evaluate, BAA enforcer MAY apply
itar_ear  -> ITAR/EAR jurisdiction MAY evaluate
ocap      -> OCAP engine MAY evaluate (Session 40)
```

Absence of a scope means the engine **must not evaluate**, not that it returns "no concern". A tenant without `itar_ear` scope handling ITAR content is itself a compliance violation — the policy runtime in Session 41 will treat scope mismatch as a `DenyExport` outcome.

### 4.4 Allowed-route semantics

| Value | Meaning |
|---|---|
| `local_only` | Hard ceiling. Even uncontrolled content stays local. |
| `local_preferred` | Local first; cloud route allowed only after classification passes. |
| `cloud_allowed` | No route ceiling from trust layer. (Compliance layer still applies.) |

A claim with `allowed_routes = { local_only }` produces `Outcome::RouteLocal` for every request, regardless of classification. This is the **air-gap-equivalent at the policy layer**.

### 4.5 Revocation status

| Value | Meaning | Effect |
|---|---|---|
| `valid` | In current snapshot and not listed as revoked | Allow processing |
| `revoked` | Explicitly revoked | `DenyExport` |
| `stale` | Snapshot is past soft expiry | Continue but warn (S43 reports must note) |
| `unknown` | No fresh snapshot available | Treat as `revoked` for ITAR; `stale` for uncontrolled content |

The conservative treatment of `unknown` for ITAR aligns with the Session 39 **default-to-ITAR-on-ambiguity** rule and the manifold's general "fail closed under uncertainty" posture.

---

## 5. Denied-Access Test Plan

These tests prove the boundaries hold. They land alongside the policy runtime in Session 41; this BF-2 spec defines what each test must demonstrate so the runtime is shaped around them.

| # | Scenario | Expected result |
|---|---|---|
| T1 | `lamprey-router` token attempts `transit/sign/lamprey-bundle-signer` | OpenBao denies (403) |
| T2 | `lamprey-dashboard` token attempts to write `kv/services/lamprey-audit/*` | OpenBao denies |
| T3 | `mai-api` token attempts to read `kv/tenants/<other_tenant>/subject-hmac` | OpenBao denies |
| T4 | A claim with `service_identity = mai-scheduler` arrives at the `lamprey-router` endpoint | Lamprey rejects (wrong identity at this surface) |
| T5 | A claim missing the `itar_ear` compliance scope is presented with ITAR content | Policy runtime returns `DenyExport`, reason `trust.scope_missing` |
| T6 | A claim with `allowed_routes = { local_only }` requests a cloud route | Policy runtime returns `RouteLocal`, reason `trust.allowed_routes` |
| T7 | A claim presented with `revocation_status = revoked` | Policy runtime returns `DenyExport`, reason `trust.revoked` |
| T8 | A claim with valid scope but `offline_mode = true` requests a cloud route | Policy runtime returns `RouteLocal`, reason `trust.offline_mode` |
| T9 | Two appliances under different tenants both attempt to mint claims via OpenBao using the same role_id | Second appliance denied; role_ids are per-appliance |
| T10 | An expired claim (past `expires_at`) is presented to the policy runtime | Policy runtime returns `DenyExport`, reason `trust.expired` |

Tests T1-T3 belong to the OpenBao integration test suite (BF-3 / BF-4 work). Tests T4-T10 belong to the policy runtime (BF-2 plumbing in `mai-compliance/src/jurisdiction.rs` covers T5-T8 today as part of this backfill; T4, T9, T10 land in S41).

---

## 6. Mapping to Existing Session 39 Code

The current `mai-compliance::jurisdiction::ActorContext` remains the regulatory-facing struct (country + person_type, used by the ITAR country/person gates). The new `mai-compliance::trust::TrustContext` is the trust-fabric struct.

The Session 39 jurisdiction evaluator takes both:

```rust
JurisdictionEvaluator::evaluate(
    &self,
    itar: &ItarReport,
    ear: &EarReport,
    actor: &ActorContext,
    trust: &TrustContext,  // new in BF-2
) -> JurisdictionDecision
```

`JurisdictionDecision` gains audit-grade trust fields (`claim_id`, `trust_bundle_version`, `service_identity`, `tenant_id`, `subject_hash`, `offline_mode`, `revocation_status`) so the BF-5 audit correlation can emit a complete record without re-deriving anything.

The mapping into the audit event is deliberately one-way: **the `subject_id` from the TrustContext never leaves the decision call frame**. Only `subject_hash` is recorded.

---

## 7. Acceptance Criteria for BF-2 (this file's share)

- [x] Each of the nine services has a named identity (§2).
- [x] No service relies on a shared broad token in the target design (§3.1).
- [x] Session 39 can receive `service_identity` via the TrustContext field (§4).
- [x] Policy runtime can later distinguish user claims from service claims (`service_identity` is `Option`, null for humans) (§4.2).
- [x] Wrong service identity can be represented and tested (denied-access plan §5, test T4).

Code-side acceptance:

- [x] `mai-compliance/src/trust.rs` defines `TrustContext`, `ServiceIdentity`, `ComplianceScope`, `AllowedRoute`, `DataClassification`, `RevocationStatus` (lands in the same backfill commit as this doc).
- [x] `JurisdictionEvaluator::evaluate` accepts a `&TrustContext` and threads its fields into `JurisdictionDecision`.
- [x] Trust-aware tests cover scope mismatch (T5), allowed-route ceiling (T6), revocation (T7), offline mode (T8), and expired claim handling (T10 covered via revocation_status = revoked + offline_mode interaction).

---

## 8. What this document does NOT cover

| Topic | Where it lives |
|---|---|
| Trust Manifold architecture and threat model | [TRUST-MANIFOLD.md](./TRUST-MANIFOLD.md) |
| OpenBao mount layout and auth method specifics | [OPENBAO-INTEGRATION.md](./OPENBAO-INTEGRATION.md) |
| Signed bundle wire format | `TRUST-BUNDLE-SPEC.md` (BF-3) |
| Local trust cache schema | `LOCAL-TRUST-CACHE.md` (BF-4) |
| Audit correlation event schema | `AUDIT-CORRELATION.md` (BF-5) |
