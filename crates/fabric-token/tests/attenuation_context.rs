//! T1/T3/T4 — VerificationContext matrix + attenuation guards.
//!
//! Exercises every context check `verify_in_context` performs (the T1 "no
//! required check can be omitted" property) and the T4 child-id / depth /
//! offline-tightening guards, all with real ML-DSA-87 signatures.

use chrono::{TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{
    Operation, TokenError, TokenRestrictions, VerificationContext, attenuate,
    attenuate_preverified, issue, verify_in_context,
};

fn base(token_id: &str, issued_at: &str, expires_at: &str) -> TrustToken {
    TrustToken {
        token_id: token_id.into(),
        issued_at: issued_at.into(),
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
        allowed_routes: vec![Route::LocalOnly, Route::LocalPreferred],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
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

#[test]
fn verify_in_context_accepts_a_valid_token() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let t = issue(
        base("t", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let ctx = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
        .expect_tenant("baap")
        .expect_bundle("2026.07.03.001");
    assert!(verify_in_context(&t, &ctx).is_ok());
    assert_eq!(ctx.operation(), Operation::Verify);
}

#[test]
fn verify_in_context_rejects_expired() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let t = issue(
        base("t", "2026-07-03T00:00:00Z", "2026-07-03T12:00:00Z"),
        &k,
    )
    .unwrap();
    let ctx = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify);
    assert_eq!(
        verify_in_context(&t, &ctx).unwrap_err(),
        TokenError::Expired
    );
}

#[test]
fn verify_in_context_rejects_not_yet_valid() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    // issued in the future relative to `now`.
    let t = issue(
        base("t", "2026-07-05T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let ctx = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify);
    assert_eq!(
        verify_in_context(&t, &ctx).unwrap_err(),
        TokenError::NotYetValid
    );
}

#[test]
fn verify_in_context_rejects_wrong_tenant_and_bundle() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let t = issue(
        base("t", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let wrong_tenant =
        VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
            .expect_tenant("other");
    assert_eq!(
        verify_in_context(&t, &wrong_tenant).unwrap_err(),
        TokenError::TenantMismatch
    );
    let wrong_bundle =
        VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
            .expect_bundle("2099.stale");
    assert_eq!(
        verify_in_context(&t, &wrong_bundle).unwrap_err(),
        TokenError::BundleMismatch
    );
}

#[test]
fn verify_in_context_fresh_revocation_rejects_unknown() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    // base() defaults revocation_status to Unknown.
    let t = issue(
        base("t", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let lax = VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify);
    assert!(
        verify_in_context(&t, &lax).is_ok(),
        "unknown tolerated by default"
    );
    let strict =
        VerificationContext::new(&MlDsa87Verifier, k.public_key(), now(), Operation::Verify)
            .require_fresh_revocation();
    assert_eq!(
        verify_in_context(&t, &strict).unwrap_err(),
        TokenError::RevocationUnknown
    );
}

#[test]
fn attenuate_rejects_empty_or_duplicate_child_id() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(
        base("parent", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        k.public_key(),
        now(),
        Operation::Attenuate,
    );
    // empty id
    assert_eq!(
        attenuate(&parent, &TokenRestrictions::new(""), &ctx, None, &k).unwrap_err(),
        TokenError::InvalidChildId
    );
    // id equal to parent (trivial cycle / duplicate)
    assert_eq!(
        attenuate(&parent, &TokenRestrictions::new("parent"), &ctx, None, &k).unwrap_err(),
        TokenError::InvalidChildId
    );
}

#[test]
fn attenuate_enforces_depth_budget() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(
        base("parent", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        k.public_key(),
        now(),
        Operation::Attenuate,
    );
    // zero depth budget refuses.
    assert_eq!(
        attenuate(&parent, &TokenRestrictions::new("c"), &ctx, Some(0), &k).unwrap_err(),
        TokenError::DepthExceeded
    );
    // a positive budget allows.
    assert!(attenuate(&parent, &TokenRestrictions::new("c"), &ctx, Some(1), &k).is_ok());
}

#[test]
fn attenuate_offline_mode_only_tightens() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(
        base("parent", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    assert!(!parent.offline_mode);
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        k.public_key(),
        now(),
        Operation::Attenuate,
    );
    let mut r = TokenRestrictions::new("c");
    r.set_offline_mode = true;
    let child = attenuate(&parent, &r, &ctx, None, &k).unwrap();
    assert!(
        child.offline_mode,
        "offline can be turned on (a tightening)"
    );
}

#[test]
fn preverified_path_still_enforces_monotonicity_and_id() {
    let k = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(
        base("parent", "2026-07-03T00:00:00Z", "2099-01-01T00:00:00Z"),
        &k,
    )
    .unwrap();
    // Valid narrowing succeeds.
    let ok = attenuate_preverified(
        &parent,
        &TokenRestrictions {
            new_token_id: "child".into(),
            allowed_routes: Some(vec![Route::LocalOnly]),
            ..TokenRestrictions::default()
        },
        now(),
        None,
        &k,
    )
    .unwrap();
    assert_eq!(
        ok.tenant_id, parent.tenant_id,
        "identity copied from parent"
    );
    // Widening still fails on the preverified path.
    let widen = attenuate_preverified(
        &parent,
        &TokenRestrictions {
            new_token_id: "child2".into(),
            roles: Some(vec!["admin".into()]),
            ..TokenRestrictions::default()
        },
        now(),
        None,
        &k,
    );
    assert_eq!(
        widen.unwrap_err(),
        TokenError::AttenuationWidens { axis: "roles" }
    );
    // Expiry can be extended past now on the preverified path? No — must be ≤ parent.
    let extend = attenuate_preverified(
        &parent,
        &TokenRestrictions {
            new_token_id: "child3".into(),
            expires_at: Some("2100-01-01T00:00:00Z".into()),
            ..TokenRestrictions::default()
        },
        now(),
        None,
        &k,
    );
    assert_eq!(
        extend.unwrap_err(),
        TokenError::AttenuationWidens { axis: "expires_at" }
    );
}
