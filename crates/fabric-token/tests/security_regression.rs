//! Adversarial regression suite — security remediation (AF-001 / AF-006).
//!
//! Each test is the deterministic regression identifier from
//! docs/scans/SECURITY-REGRESSION-REGISTRY.md. At M0 these were feature-gated and
//! asserted the VULNERABLE behavior of `attenuate` (the signer oracle + missing
//! monotonicity). The AF-001 fix has since landed, so each has been FLIPPED to
//! assert the repaired behavior and de-quarantined into the default product
//! suite — a permanent guard that the signer-oracle and widening holes stay shut.
//!
//! Run: `cargo test -p fabric-token`

use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{TokenError, attenuate, issue};

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

// REG-AF-001-unsigned-parent — a fabricated, never-signed parent must NOT yield a
// signed child. `attenuate` verifies the parent first, closing the signer oracle.
#[test]
fn reg_af_001_unsigned_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = base("forged_parent", "2099-01-01T00:00:00Z");
    assert!(parent.signature.value.is_empty(), "parent is unsigned");
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::CloudAllowed];
    // FIXED: the unsigned parent fails verification; no child is minted.
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::InvalidSignature),
    );
}

// REG-AF-001-wrong-key-parent — a parent signed by an attacker key (not the bridge
// anchor) must be rejected: `attenuate` verifies the parent under the bridge anchor.
#[test]
fn reg_af_001_wrong_key_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let parent = issue(base("attacker_parent", "2099-01-01T00:00:00Z"), &attacker).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    // FIXED: verified under the bridge anchor, the attacker-key parent fails.
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::InvalidSignature),
    );
}

// REG-AF-001-role-widening — a child that gains a role the parent never held must
// be rejected: `roles` must be a subset of the parent's.
#[test]
fn reg_af_001_role_widening_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.roles = vec!["clinician".into(), "admin".into()];
    // FIXED: "admin" is not held by the parent -> widening on the roles axis.
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::AttenuationWidens { axis: "roles" }),
    );
}

// REG-AF-001-tenant-swap — a child that claims a different tenant must be rejected:
// `tenant_id` is immutable under attenuation.
#[test]
fn reg_af_001_tenant_swap_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.tenant_id = "victim-tenant".into();
    // FIXED: the tenant swap is a widening on the tenant_id axis.
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::AttenuationWidens { axis: "tenant_id" }),
    );
}

// REG-AF-006-revoked-parent — a revoked parent must NOT mint fresh children:
// `attenuate` checks the parent's revocation status before signing anything.
#[test]
fn reg_af_006_revoked_parent_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let mut p = base("parent", "2099-01-01T00:00:00Z");
    p.revocation_status = RevocationStatus::Revoked;
    let parent = issue(p, &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    // FIXED: the revoked parent fails verification; no child is minted.
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::Revoked),
    );
}

// REG-AF-001-service-identity-swap — a child that assumes a different service
// identity must be rejected: service identity is immutable under attenuation.
#[test]
fn reg_af_001_service_identity_swap_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.service_identity = Some("aog-controller".into());
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::AttenuationWidens {
            axis: "service_identity"
        }),
    );
}

// REG-AF-001-scope-widening — a child that gains a compliance scope the parent
// never held must be rejected: scopes must be a subset of the parent's.
#[test]
fn reg_af_001_scope_widening_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.compliance_scopes = vec![ComplianceScope::Hipaa, ComplianceScope::ItarEar];
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::AttenuationWidens {
            axis: "compliance_scopes"
        }),
    );
}

// REG-AF-001-subject-swap — a child that claims a different subject must be
// rejected: the subject binding is immutable under attenuation.
#[test]
fn reg_af_001_subject_swap_is_rejected() {
    let bridge = RustCryptoMlDsa87::generate("bridge").unwrap();
    let parent = issue(base("parent", "2099-01-01T00:00:00Z"), &bridge).unwrap();
    let mut child = base("child", "2098-01-01T00:00:00Z");
    child.allowed_routes = vec![Route::LocalOnly];
    child.subject_hash = "hmac:someone-else".into();
    assert_eq!(
        attenuate(
            &parent,
            child,
            &bridge,
            &MlDsa87Verifier,
            bridge.public_key()
        ),
        Err(TokenError::AttenuationWidens {
            axis: "subject_hash"
        }),
    );
}
