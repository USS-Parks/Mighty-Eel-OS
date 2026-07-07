//! Adversarial regression harness — AF-001 / AF-006 (Phase T, repaired).
//!
//! Each test is the deterministic regression identifier from
//! docs/scans/SECURITY-REGRESSION-REGISTRY.md. They were frozen against the
//! vulnerable behavior in prompt 0.4; Phase T flipped them to assert the
//! **repaired** behavior — a fabricated / wrong-key / expired / revoked parent,
//! or a child that widens any authority axis, is now refused, never signed — so
//! they run in the default product suite (no feature gate) as live guards.

use chrono::Utc;
use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{TokenError, VerificationContext, attenuate, issue};

/// A maximally-permissive base token; each fixture narrows it as needed. Empty
/// `allowed_models` and `None` budget make attenuate skip those two subset checks,
/// isolating the axis each fixture exercises.
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

/// A context anchoring the parent on `bridge`'s key at the current time.
fn ctx_for<'a>(bridge: &'a RustCryptoMlDsa87) -> VerificationContext<'a> {
    VerificationContext::new(&MlDsa87Verifier, bridge.public_key(), Utc::now())
}

// REG-AF-001-unsigned-parent — a fabricated, never-signed parent can no longer
// mint a signed child: the signer oracle is closed.
#[test]
fn reg_af_001_unsigned_parent_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = base("forged_parent", "2099-01-01T00:00:00Z");
    assert!(parent.signature.value.is_empty(), "parent is unsigned");
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::CloudAllowed];
    let err = attenuate(&parent, child, &ctx_for(&bridge), &bridge).unwrap_err();
    assert!(
        matches!(err, TokenError::ParentUnverified(_)),
        "unsigned parent must be refused, got {err:?}"
    );
}

// REG-AF-001-wrong-key-parent — a parent signed by an attacker key (not the
// bridge anchor) is refused; attenuate verifies the parent signature.
#[test]
fn reg_af_001_wrong_key_parent_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let parent = issue(base("attacker_parent", "2099-01-01T00:00:00Z"), &attacker).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    let err = attenuate(&parent, child, &ctx_for(&bridge), &bridge).unwrap_err();
    assert!(
        matches!(err, TokenError::ParentUnverified(_)),
        "wrong-key parent must be refused, got {err:?}"
    );
}

// REG-AF-001-role-widening — a child that gains a role the parent never held is
// refused; roles must be a subset.
#[test]
fn reg_af_001_role_widening_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.roles = vec!["clinician".into(), "admin".into()]; // parent holds only clinician
    let err = attenuate(&parent, child, &ctx_for(&bridge), &bridge).unwrap_err();
    assert_eq!(err, TokenError::AttenuationWidens { axis: "roles" });
}

// REG-AF-001-tenant-swap — a child that claims a different tenant is refused;
// tenant identity is immutable across attenuation.
#[test]
fn reg_af_001_tenant_swap_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.tenant_id = "victim-tenant".into();
    let err = attenuate(&parent, child, &ctx_for(&bridge), &bridge).unwrap_err();
    assert_eq!(err, TokenError::AttenuationWidens { axis: "tenant_id" });
}

// REG-AF-006-revoked-parent — a revoked parent can no longer mint fresh
// children: attenuate checks the parent's revocation status first.
#[test]
fn reg_af_006_revoked_parent_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let mut p = base("parent", "2099-01-01T00:00:00Z");
    p.revocation_status = RevocationStatus::Revoked;
    let parent = issue(p, &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    let err = attenuate(&parent, child, &ctx_for(&bridge), &bridge).unwrap_err();
    assert_eq!(err, TokenError::ParentRevoked);
}
