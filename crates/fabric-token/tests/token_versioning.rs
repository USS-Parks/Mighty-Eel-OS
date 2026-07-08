//! T6 — token v1 compatibility: production deny-by-default, bounded verify-only
//! migration, and no v1 attenuation.

use chrono::{TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{
    Operation, TokenError, TokenRestrictions, VerificationContext, attenuate, issue,
    verify_in_context,
};

const CURRENT: &str = "2026.07.v2";
const LEGACY: &str = "2025.01.v1";

fn token(bundle: &str) -> TrustToken {
    TrustToken {
        token_id: "t".into(),
        issued_at: "2026-07-03T00:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: bundle.into(),
        tenant_id: "baap".into(),
        subject_id: None,
        subject_hash: "hmac:abc".into(),
        service_identity: None,
        identity_id: None,
        roles: vec!["clinician".into()],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
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

#[test]
fn current_bundle_verifies_legacy_is_denied_by_default() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let v2 = issue(token(CURRENT), &k).unwrap();
    let v1 = issue(token(LEGACY), &k).unwrap();

    let ctx = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
        .require_current_bundle(CURRENT);

    // v2 verifies.
    assert!(verify_in_context(&v2, &ctx).is_ok());
    // v1 is denied by default (production downgrade attempt fails).
    assert_eq!(
        verify_in_context(&v1, &ctx).unwrap_err(),
        TokenError::UnsupportedTokenVersion(LEGACY.into())
    );
    assert!(ctx.is_legacy(&v1));
    assert!(!ctx.is_legacy(&v2));
}

#[test]
fn legacy_verifies_only_under_bounded_migration_flag() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let v1 = issue(token(LEGACY), &k).unwrap();

    let migrate =
        VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
            .require_current_bundle(CURRENT)
            .permit_legacy_verify();
    // With the explicit migration flag, a v1 token may verify (verify-only path).
    assert!(verify_in_context(&v1, &migrate).is_ok());
}

#[test]
fn legacy_is_never_an_attenuation_parent() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let v1 = issue(token(LEGACY), &k).unwrap();

    // Even with the migration flag that permits verify, attenuation of a v1
    // parent is refused — no v1 attenuation, ever.
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        k.public_key(),
        now(),
        Operation::Attenuate,
    )
    .require_current_bundle(CURRENT)
    .permit_legacy_verify();
    assert_eq!(
        attenuate(&v1, &TokenRestrictions::new("child"), &ctx, None, &k).unwrap_err(),
        TokenError::LegacyAttenuationDenied(LEGACY.into())
    );
}

#[test]
fn no_version_policy_is_backward_compatible() {
    // Without require_current_bundle, any bundle is accepted (no gating) — the
    // pre-T6 behavior for callers that have not opted into the version policy.
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let v1 = issue(token(LEGACY), &k).unwrap();
    let ctx = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify);
    assert!(!ctx.is_legacy(&v1));
    assert!(verify_in_context(&v1, &ctx).is_ok());
}
