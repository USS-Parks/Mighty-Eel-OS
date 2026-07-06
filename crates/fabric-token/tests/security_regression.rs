//! Quarantined adversarial regression harness — security remediation.
//!
//! Feature-gated (`security-regression`) so it never runs in the default suite:
//! every test here asserts the CURRENT, VULNERABLE behavior of the trust plane.
//! When a fix lands, the matching test flips to assert the repaired behavior and
//! moves into the product suite. Each test name is the deterministic regression
//! identifier (see docs/scans/SECURITY-REGRESSION-REGISTRY.md).
//!
//! Run: `cargo test -p fabric-token --features security-regression`
#![cfg(feature = "security-regression")]

use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{attenuate, issue, verify};

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

// REG-AF-001-unsigned-parent — attenuate signs a child of a fabricated, never-
// signed parent. The parent is trusted without verification: a signer oracle.
#[test]
fn reg_af_001_unsigned_parent_yields_signed_child() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = base("forged_parent", "2099-01-01T00:00:00Z");
    assert!(parent.signature.value.is_empty(), "parent is unsigned");
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::CloudAllowed];
    // VULNERABLE today: succeeds. The fix must verify the parent and reject.
    let signed = attenuate(&parent, child, &bridge)
        .expect("VULN(AF-001): unsigned parent accepted, child signed");
    verify(&signed, &MlDsa87Verifier, bridge.public_key())
        .expect("VULN(AF-001): forged-lineage child verifies under the bridge key");
}

// REG-AF-001-wrong-key-parent — a parent signed by an attacker key (not the
// bridge anchor) is accepted; attenuate never checks the parent signature.
#[test]
fn reg_af_001_wrong_key_parent_yields_signed_child() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let parent = issue(base("attacker_parent", "2099-01-01T00:00:00Z"), &attacker).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    let signed =
        attenuate(&parent, child, &bridge).expect("VULN(AF-001): wrong-key parent accepted");
    verify(&signed, &MlDsa87Verifier, bridge.public_key()).unwrap();
}

// REG-AF-001-role-widening — the child gains a role the parent never held;
// attenuate does not constrain `roles`.
#[test]
fn reg_af_001_role_widening_accepted() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.roles = vec!["clinician".into(), "admin".into()];
    let signed = attenuate(&parent, child, &bridge).expect("VULN(AF-001): role widening accepted");
    assert!(signed.roles.iter().any(|r| r == "admin"));
}

// REG-AF-001-tenant-swap — the child claims a different tenant; attenuate does
// not constrain `tenant_id`.
#[test]
fn reg_af_001_tenant_swap_accepted() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.tenant_id = "victim-tenant".into();
    let signed =
        attenuate(&parent, child, &bridge).expect("VULN(AF-001): cross-tenant child accepted");
    assert_eq!(signed.tenant_id, "victim-tenant");
}

// REG-AF-006-revoked-parent — attenuate never checks the parent's revocation
// status, so a revoked token still mints fresh, non-revoked children.
#[test]
fn reg_af_006_revoked_parent_still_attenuates() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let mut p = base("parent", "2099-01-01T00:00:00Z");
    p.revocation_status = RevocationStatus::Revoked;
    let parent = issue(p, &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    let signed =
        attenuate(&parent, child, &bridge).expect("VULN(AF-006): revoked parent still attenuates");
    assert_eq!(signed.revocation_status, RevocationStatus::Unknown);
}
