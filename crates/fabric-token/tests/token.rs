//! fabric-token tests (Prompt F3): issue/verify round-trip, tamper rejection,
//! the attenuation narrowing invariant (both directions), budget metering, and
//! expiry.

use chrono::{TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, ComplianceScope, RevocationStatus, Route, Signature,
    TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_token::{
    TokenError, VerificationContext, attenuate, is_expired, issue, try_spend, verify,
};

fn base_token(expires_at: &str) -> TrustToken {
    TrustToken {
        token_id: "tok_1".into(),
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
        allowed_routes: vec![Route::LocalOnly, Route::LocalPreferred],
        allowed_models: vec!["llama-3-8b".into(), "mistral-7b".into()],
        max_data_classification: Classification::Restricted,
        country: Some("US".into()),
        person_type: Some("us_person".into()),
        offline_mode: false,
        revocation_status: RevocationStatus::Unknown,
        budget: Some(Budget {
            token_cap: 1000,
            tokens_spent: 0,
            usd_cap_cents: 500,
            usd_spent_cents: 0,
            tool_call_cap: 10,
            tool_calls_spent: 0,
        }),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    }
}

#[test]
fn issue_then_verify_round_trip() {
    let signer = RustCryptoMlDsa87::generate("bridge-q3").unwrap();
    let minted = issue(base_token("2099-01-01T00:00:00Z"), &signer).unwrap();
    assert_eq!(minted.signature.alg, "ml-dsa-87");
    assert!(!minted.signature.value.is_empty());
    verify(&minted, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn tampered_token_fails_verification() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let mut minted = issue(base_token("2099-01-01T00:00:00Z"), &signer).unwrap();
    minted.tenant_id = "other-tenant".into(); // mutate a minted field
    assert_eq!(
        verify(&minted, &MlDsa87Verifier, signer.public_key()),
        Err(TokenError::InvalidSignature)
    );
}

#[test]
fn revoked_token_is_rejected() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let mut t = base_token("2099-01-01T00:00:00Z");
    t.revocation_status = RevocationStatus::Revoked;
    let minted = issue(t, &signer).unwrap();
    assert_eq!(
        verify(&minted, &MlDsa87Verifier, signer.public_key()),
        Err(TokenError::Revoked)
    );
}

#[test]
fn attenuate_narrows_and_binds_parent() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(base_token("2099-01-01T00:00:00Z"), &signer).unwrap();

    let mut child = base_token("2098-01-01T00:00:00Z"); // earlier expiry
    child.token_id = "tok_child".into();
    child.allowed_routes = vec![Route::LocalOnly]; // subset
    child.allowed_models = vec!["llama-3-8b".into()]; // subset
    child.max_data_classification = Classification::Internal; // lower
    child.budget = Some(Budget {
        token_cap: 100,
        tokens_spent: 0,
        usd_cap_cents: 50,
        usd_spent_cents: 0,
        tool_call_cap: 2,
        tool_calls_spent: 0,
    });

    let ctx = VerificationContext::new(&MlDsa87Verifier, signer.public_key(), Utc::now());
    let minted_child = attenuate(&parent, child, &ctx, &signer).unwrap();
    assert_eq!(minted_child.attenuation.parent_id.as_deref(), Some("tok_1"));
    assert_eq!(minted_child.attenuation.depth, 1);
    verify(&minted_child, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn attenuate_rejects_widening() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = issue(base_token("2099-01-01T00:00:00Z"), &signer).unwrap();
    let ctx = VerificationContext::new(&MlDsa87Verifier, signer.public_key(), Utc::now());

    // Widen routes: CloudAllowed is not in the parent.
    let mut widen_route = base_token("2098-01-01T00:00:00Z");
    widen_route.token_id = "tok_child".into();
    widen_route.allowed_routes = vec![Route::CloudAllowed];
    assert_eq!(
        attenuate(&parent, widen_route, &ctx, &signer).unwrap_err(),
        TokenError::AttenuationWidens {
            axis: "allowed_routes"
        }
    );

    // Widen classification: Secret > Restricted.
    let mut widen_class = base_token("2098-01-01T00:00:00Z");
    widen_class.token_id = "tok_child".into();
    widen_class.allowed_routes = vec![Route::LocalOnly];
    widen_class.max_data_classification = Classification::Secret;
    assert_eq!(
        attenuate(&parent, widen_class, &ctx, &signer).unwrap_err(),
        TokenError::AttenuationWidens {
            axis: "max_data_classification"
        }
    );

    // Widen budget: child cap exceeds parent remaining.
    let mut widen_budget = base_token("2098-01-01T00:00:00Z");
    widen_budget.token_id = "tok_child".into();
    widen_budget.allowed_routes = vec![Route::LocalOnly];
    widen_budget.budget = Some(Budget {
        token_cap: 5000, // > parent's 1000 remaining
        tokens_spent: 0,
        usd_cap_cents: 10,
        usd_spent_cents: 0,
        tool_call_cap: 1,
        tool_calls_spent: 0,
    });
    assert_eq!(
        attenuate(&parent, widen_budget, &ctx, &signer).unwrap_err(),
        TokenError::AttenuationWidens { axis: "budget" }
    );
}

#[test]
fn budget_meters_and_stops_at_cap() {
    let mut t = base_token("2099-01-01T00:00:00Z");
    // Spend within caps.
    try_spend(&mut t, 400, 200, 4).unwrap();
    try_spend(&mut t, 500, 200, 4).unwrap();
    let b = t.budget.as_ref().unwrap();
    assert_eq!(b.tokens_spent, 900);
    assert_eq!(b.tool_calls_spent, 8);
    // Next token spend would exceed the 1000 cap.
    assert_eq!(
        try_spend(&mut t, 200, 0, 0).unwrap_err(),
        TokenError::BudgetExceeded { counter: "tokens" }
    );
    // The failed spend left counters unchanged.
    assert_eq!(t.budget.as_ref().unwrap().tokens_spent, 900);
}

#[test]
fn no_budget_means_metering_is_a_noop() {
    let mut t = base_token("2099-01-01T00:00:00Z");
    t.budget = None;
    try_spend(&mut t, u64::MAX, u64::MAX, u32::MAX).unwrap();
}

#[test]
fn expiry_check() {
    let t = base_token("2026-07-03T18:15:00Z");
    let before = Utc.with_ymd_and_hms(2026, 7, 3, 18, 0, 0).unwrap();
    let after = Utc.with_ymd_and_hms(2026, 7, 3, 18, 30, 0).unwrap();
    assert!(!is_expired(&t, before).unwrap());
    assert!(is_expired(&t, after).unwrap());
}
