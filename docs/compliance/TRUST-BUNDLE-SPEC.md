# Trust Bundle Specification (BF-3)

**Status:** Backfill BF-3 of the Trust Manifold lane (BUILD-EXECUTION-PLAN
Appendix A §A.7). Gates Session 41.

**Purpose:** Define the on-the-wire shape and local verification rules for
signed policy bundles and signed claims, so the policy runtime (Session 41)
can reject unsigned or invalid material without ambiguity.

This document is the wire-format companion to [TRUST-MANIFOLD.md](TRUST-MANIFOLD.md)
(architecture), [OPENBAO-INTEGRATION.md](../operations/OPENBAO-INTEGRATION.md) (issuer side),
and [LOCAL-TRUST-CACHE.md](LOCAL-TRUST-CACHE.md) (cache state machine).

---

## 1. Signature primitives

| Primitive   | Algorithm        | Source                                                   |
|-------------|------------------|----------------------------------------------------------|
| Bundle sig  | ML-DSA-87        | `mai-vault::pqc` (FIPS 204; pqc-dev or pqc-prod backend) |
| Claim sig   | ML-DSA-87        | Same engine; per-tenant signing key                      |
| Subject hash| HMAC-SHA256      | Per-tenant 32-byte HMAC key                              |
| Hashing     | SHA-256, BLAKE3  | SHA-256 for canonical-JSON digest; BLAKE3 internal       |

Choice of ML-DSA-87 matches the audit-chain checkpoint signing introduced in
S27 (`AuditWriter::with_pqc`). One PQC primitive across the appliance keeps
the threat model tractable.

---

## 2. Signed Policy Bundle

A policy bundle is the unit the Trust Bridge ships to a local appliance
when its trust cache refreshes. One bundle carries one snapshot of the
revocation list plus metadata about which policy version it embodies.

### 2.1 Wire shape (canonical JSON)

```json
{
  "metadata": {
    "version": "2026.05.22.001",
    "issuer": "trust-bridge",
    "issued_at_secs": 1747958400,
    "expires_at_secs": 1748044800,
    "tenant_id": "tribal-health-demo"
  },
  "payload": {
    "revocations": [
      { "claim_id": "claim-001", "status": "valid",   "recorded_at_secs": 1747958400 },
      { "claim_id": "claim-002", "status": "revoked", "recorded_at_secs": 1747958400 }
    ]
  },
  "signature": {
    "algorithm": "ml-dsa-87",
    "public_key_id": "trust-bridge-2026-q2",
    "bytes_hex": "..."
  }
}
```

### 2.2 Canonicalization rule

The signed payload is the BLAKE3 hash of the canonical-JSON encoding of
`{ "metadata": ..., "payload": ... }` — signature stripped, keys sorted
lexicographically, no extraneous whitespace. The Rust impl uses
`serde_json::to_string` over a `BTreeMap`-backed projection to guarantee
ordering; the corresponding Trust Bridge implementation MUST match.

The wire bundle includes the signature alongside the payload; the receiver
strips the signature, re-canonicalizes, hashes, and verifies the signature
against that hash.

---

## 3. Signed Claim

A claim is a single subject-level assertion. Claims are issued per-tenant
and carry the same envelope as a bundle, but the payload is the trust
context projection rather than a revocation list.

### 3.1 Wire shape

```json
{
  "metadata": {
    "version": "claim-2026.05.22.001",
    "issuer": "trust-bridge",
    "issued_at_secs": 1747958400,
    "expires_at_secs": 1747962000,
    "tenant_id": "tribal-health-demo"
  },
  "payload": {
    "claim_id": "claim-001",
    "subject_hash": "hmac:9f3c...e21",
    "service_identity": "lamprey-router",
    "compliance_scopes": ["hipaa", "ocap"],
    "allowed_routes": ["local-only"],
    "data_classification": "phi"
  },
  "signature": {
    "algorithm": "ml-dsa-87",
    "public_key_id": "tenant-tribal-health-2026-q2",
    "bytes_hex": "..."
  }
}
```

Claims are short-lived (1-hour TTL is the default). The trust cache holds
only the most recent valid claim per subject.

---

## 4. Local verification design

### 4.1 Inputs

- The wire bundle / claim (decoded from JSON).
- A `TrustAnchor` registry mapping `public_key_id` to a vetted ML-DSA-87
  public key. The registry is populated at first-boot from configuration
  and rotated under operator control.
- Wall-clock `now_secs`.

### 4.2 Algorithm

```
verify(bundle, anchor_registry, now_secs):
    if bundle.metadata.expires_at_secs <= now_secs:
        return Err(Expired)
    if bundle.metadata.issued_at_secs > now_secs:
        return Err(NotYetValid)
    anchor = anchor_registry.get(bundle.signature.public_key_id)
    if anchor is None:
        return Err(MissingTrustAnchor)
    payload_hash = blake3(canonical_json({metadata, payload}))
    if not ml_dsa_87.verify(payload_hash, bundle.signature.bytes, anchor):
        return Err(InvalidSignature)
    return Ok(())
```

### 4.3 Rust trait

`BundleVerifier` (in `mai-compliance::bundle`) is a single-method trait
that takes the canonicalized payload hash and the signature, and returns
`Result<(), BundleError>`. The default impl `MlDsaBundleVerifier` wraps an
in-memory anchor registry and uses the `ml-dsa` crate to verify. Tests
substitute `AcceptAllBundleVerifier` or `RejectAllBundleVerifier` as
needed without pulling crypto into the test path.

---

## 5. Failure-mode behaviors

| Failure              | Code path returns      | Trust cache action          | Audit record |
|----------------------|------------------------|-----------------------------|--------------|
| Bundle expired       | `BundleError::Expired` | refresh refused; cache aged | yes          |
| Bundle not-yet-valid | `BundleError::NotYetValid` | refresh refused          | yes          |
| Signature invalid    | `BundleError::InvalidSignature` | refresh refused; tamper-alarm flag | yes  |
| Unknown anchor       | `BundleError::MissingTrustAnchor` | refresh refused      | yes (config drift) |
| Malformed JSON       | `BundleError::Malformed` | refresh refused; surfaced | yes        |
| Future timestamp on `issued_at` | `BundleError::NotYetValid` | refused (clock-skew suspect) | yes |

In every failure case the existing trust cache state is **preserved**. A
failed refresh does not clobber a valid cache; the cache ages naturally
and eventually hits its `StaleNotExpired` then `Expired` thresholds per
the BF-4 state machine.

---

## 6. HMAC subject hashing

### 6.1 Why

Raw subject identifiers (employee IDs, patient MRNs, treaty signatories,
service accounts) cannot appear in audit logs that may sync to a central
audit store. HMAC pseudonymization gives a stable, per-tenant identifier
that supports correlation without disclosure.

### 6.2 Construction

```
subject_hash(tenant_key, subject_id) = "hmac:" + lowercase_hex(HMAC-SHA256(tenant_key, subject_id))
```

- `tenant_key` is a 32-byte per-tenant key held in the local vault. It
  never leaves the appliance.
- The `"hmac:"` prefix marks the string as a pseudonymized identifier in
  audit records; raw subject IDs MUST NEVER appear without this prefix.
- The same `tenant_key` always yields the same hash for the same
  `subject_id` — required for cross-event correlation within a tenant.
- A different tenant's key yields a different hash — cross-tenant
  correlation is intentionally impossible.

### 6.3 Rotation

Tenant HMAC keys rotate on the same cadence as the tenant's ML-DSA
signing key (default: quarterly). Rotation breaks historical correlation
within a tenant; audit consumers that need long-running correlation must
read both the current and previous quarter's keys. The rotation policy
is owned by Session 43 audit; this spec only defines the format.

---

## 7. Compatibility with the existing trust cache

The S28/BF-4 `LocalTrustCache::record_refresh(version, snapshots, refresh_secs, now_secs)`
remains the bare-data entry point for tests and trusted in-process callers
(e.g., during first-boot bootstrap).

BF-3 adds `LocalTrustCache::record_signed_refresh(bundle, verifier, now_secs)`
which verifies the bundle, then on success delegates to `record_refresh`.
Production code paths in Session 41 use the signed entry point only.

---

## 8. Acceptance criteria mapping (Appendix A §A.7)

- [x] Policy runtime can record `trust_bundle_version` — already supported by
      `LocalTrustCache::bundle_version()` and `TrustContext::trust_bundle_version`.
- [x] Policy runtime can reject unsigned or invalid bundles — `record_signed_refresh`
      returns `BundleError` and leaves cache state unchanged on failure.
- [x] Audit events can include `claim_id` and `trust_bundle_version` — both fields
      already surface in `TrustSnapshot` (S39 BF-2 wiring).
- [x] Subject identity can be pseudonymized by HMAC — `mai_compliance::subject_hash::hmac_subject`.
- [x] Session 41 does not hardcode unsigned local policy as the only long-term path
      — the signed refresh entry point is the canonical production path.
