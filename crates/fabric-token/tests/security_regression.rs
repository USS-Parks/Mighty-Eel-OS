//! Adversarial regression suite — security remediation (AF-001 / AF-006).
//!
//! These began (plan 0.4) as quarantined fixtures asserting the CURRENT,
//! VULNERABLE behavior. Phase T landed the fix, so each has flipped to assert
//! the REPAIRED behavior and moved into the product suite (no feature gate).
//! Each test name is the deterministic regression identifier (see
//! docs/scans/SECURITY-REGRESSION-REGISTRY.md).

use chrono::{TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{
    Operation, TokenError, TokenRestrictions, VerificationContext, attenuate, issue,
};

/// A maximally-permissive base token; each fixture narrows it as needed.
fn base(token_id: &str, expires_at: &str) -> TrustToken {
    TrustToken {
        token_id: token_id.into(),
        issued_at: "2026-07-03T18:00:00Z".into(),
        expires_at: expires_at.into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: "2026.07.03.001".into(),
        tenant_id: "baap".into(),
        subject_id: None,
        subject_hash: "hmac:abc".into(),
        service_identity: Some("aog-gateway".into()),
        identity_id: Some("id_1".into()),
        roles: vec!["clinician".into()],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![Route::LocalOnly, Route::LocalPreferred, Route::CloudAllowed],
        allowed_models: vec![],
        max_data_classification: Classification::Secret,
        country: Some("US".into()),
        person_type: Some("us_person".into()),
        offline_mode: false,
        revocation_status: RevocationStatus::Unknown,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    }
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap()
}

/// A minimal, valid narrowing (routes ⊆ parent) — isolates the axis each
/// fixture actually exercises.
fn narrow(child_id: &str) -> TokenRestrictions {
    TokenRestrictions {
        new_token_id: child_id.into(),
        allowed_routes: Some(vec![Route::LocalOnly]),
        ..TokenRestrictions::default()
    }
}

// REG-AF-001-unsigned-parent — an unsigned, fabricated parent is REJECTED: the
// signer oracle is closed because the parent is authenticated first.
#[test]
fn reg_af_001_unsigned_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = base("forged_parent", "2099-01-01T00:00:00Z");
    assert!(parent.signature.value.is_empty(), "parent is unsigned");
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        bridge.public_key(),
        now(),
        Operation::Attenuate,
    );
    assert_eq!(
        attenuate(&parent, &narrow("child"), &ctx, None, &bridge).unwrap_err(),
        TokenError::InvalidSignature
    );
}

// REG-AF-001-wrong-key-parent — a parent signed by an attacker key (not the
// bridge anchor) is REJECTED.
#[test]
fn reg_af_001_wrong_key_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let parent = issue(base("attacker_parent", "2099-01-01T00:00:00Z"), &attacker).unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        bridge.public_key(),
        now(),
        Operation::Attenuate,
    );
    assert_eq!(
        attenuate(&parent, &narrow("child"), &ctx, None, &bridge).unwrap_err(),
        TokenError::InvalidSignature
    );
}

// REG-AF-001-role-widening — a child cannot gain a role the parent never held.
#[test]
fn reg_af_001_role_widening_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        bridge.public_key(),
        now(),
        Operation::Attenuate,
    );
    let mut r = narrow("child");
    r.roles = Some(vec!["clinician".into(), "admin".into()]);
    assert_eq!(
        attenuate(&parent, &r, &ctx, None, &bridge).unwrap_err(),
        TokenError::AttenuationWidens { axis: "roles" }
    );
}

// REG-AF-001-tenant-swap — the child's tenant is now STRUCTURALLY inherited from
// the authenticated parent (the restriction schema has no tenant field), so a
// cross-tenant child is impossible to express. A valid narrowing keeps the
// parent's tenant.
#[test]
fn reg_af_001_tenant_is_inherited_not_settable() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        bridge.public_key(),
        now(),
        Operation::Attenuate,
    );
    let child = attenuate(&parent, &narrow("child"), &ctx, None, &bridge).unwrap();
    assert_eq!(
        child.tenant_id, "baap",
        "child inherits the parent's tenant"
    );
    assert_eq!(child.subject_hash, parent.subject_hash);
    assert_eq!(child.issuer, parent.issuer);
}

// REG-AF-006-revoked-parent — a revoked parent is REJECTED before any child is
// constructed.
#[test]
fn reg_af_006_revoked_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let mut p = base("parent", "2099-01-01T00:00:00Z");
    p.revocation_status = RevocationStatus::Revoked;
    let parent = issue(p, &bridge).unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        bridge.public_key(),
        now(),
        Operation::Attenuate,
    );
    assert_eq!(
        attenuate(&parent, &narrow("child"), &ctx, None, &bridge).unwrap_err(),
        TokenError::Revoked
    );
}
