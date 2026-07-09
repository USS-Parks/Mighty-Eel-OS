//! The complete revocation predicate, honored by the shared
//! verification path. A real ML-DSA-signed snapshot revokes a token on every
//! supported dimension, and `verify_in_context` (hence `attenuate` and any
//! consumer using it) denies it. A stale snapshot fails closed.

use chrono::{Duration, TimeZone, Utc};
use fabric_contracts::{
    Attenuation, Classification, ComplianceScope, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::RevocationSnapshot;
use fabric_token::{
    Operation, TokenError, TokenRestrictions, VerificationContext, attenuate, issue,
    verify_in_context,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap()
}

fn token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let t = TrustToken {
        token_id: "tok-r".into(),
        issued_at: "2026-07-03T00:00:00Z".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        issuer: "wsf-bridge".into(),
        trust_bundle_version: "2026.07.v2".into(),
        tenant_id: "tenant-a".into(),
        subject_id: None,
        subject_hash: "hmac:subj".into(),
        service_identity: Some("aog-gateway".into()),
        identity_id: None,
        roles: vec!["clinician".into()],
        compliance_scopes: vec![ComplianceScope::Hipaa],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    issue(t, signer).unwrap()
}

/// A fresh (unexpired) snapshot the caller has already signed + verified.
fn snapshot(anchor: &RustCryptoMlDsa87) -> RevocationSnapshot {
    let s = RevocationSnapshot::new(
        "snap-r",
        now().to_rfc3339(),
        (now() + Duration::hours(1)).to_rfc3339(),
    );
    fabric_revocation::sign(s, anchor).unwrap()
}

fn ctx_with<'a>(key: &'a [u8], snap: &'a RevocationSnapshot) -> VerificationContext<'a> {
    VerificationContext::new(&MlDsa87Verifier, key, now(), Operation::Verify).with_revocation(snap)
}

#[test]
fn a_valid_token_passes_when_the_snapshot_does_not_name_it() {
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let t = token(&anchor);
    let snap = snapshot(&anchor);
    assert!(verify_in_context(&t, &ctx_with(anchor.public_key(), &snap)).is_ok());
    assert_eq!(snap.revokes(&t), None);
}

fn base_snap() -> RevocationSnapshot {
    RevocationSnapshot::new(
        "snap",
        now().to_rfc3339(),
        (now() + Duration::hours(1)).to_rfc3339(),
    )
}

/// Assert a snapshot revoking one dimension both matches the predicate and is
/// denied by the shared verification path.
fn assert_denies(anchor: &RustCryptoMlDsa87, t: &TrustToken, label: &str, s: RevocationSnapshot) {
    let signed = fabric_revocation::sign(s, anchor).unwrap();
    assert!(
        signed.revokes(t).is_some(),
        "predicate should catch {label}"
    );
    assert_eq!(
        verify_in_context(t, &ctx_with(anchor.public_key(), &signed)).unwrap_err(),
        TokenError::Revoked,
        "verify_in_context must deny a token revoked by {label}"
    );
}

#[test]
fn every_dimension_revokes_the_token() {
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let t = token(&anchor);
    let key_id = t.signature.key_id.clone();

    let mut s = base_snap();
    s.revoked_tokens.push("tok-r".into());
    assert_denies(&anchor, &t, "token_id", s);

    let mut s = base_snap();
    s.revoked_subjects.push("hmac:subj".into());
    assert_denies(&anchor, &t, "subject", s);

    let mut s = base_snap();
    s.revoked_signing_keys.push(key_id);
    assert_denies(&anchor, &t, "signing_key", s);

    let mut s = base_snap();
    s.revoked_issuers.push("wsf-bridge".into());
    assert_denies(&anchor, &t, "issuer", s);

    let mut s = base_snap();
    s.revoked_bundle_versions.push("2026.07.v2".into());
    assert_denies(&anchor, &t, "bundle", s);

    let mut s = base_snap();
    s.revoked_tenants.push("tenant-a".into());
    assert_denies(&anchor, &t, "tenant", s);

    let mut s = base_snap();
    s.revoked_service_identities.push("aog-gateway".into());
    assert_denies(&anchor, &t, "service_identity", s);
}

#[test]
fn a_revoked_token_cannot_be_attenuated() {
    // Through a real consumer (attenuate): a token revoked by the snapshot is
    // refused before any child is minted.
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let parent = token(&anchor);
    let mut s = RevocationSnapshot::new(
        "snap",
        now().to_rfc3339(),
        (now() + Duration::hours(1)).to_rfc3339(),
    );
    s.revoked_tenants.push("tenant-a".into());
    let signed = fabric_revocation::sign(s, &anchor).unwrap();
    let ctx = VerificationContext::new(
        &MlDsa87Verifier,
        anchor.public_key(),
        now(),
        Operation::Attenuate,
    )
    .with_revocation(&signed);
    assert_eq!(
        attenuate(
            &parent,
            &TokenRestrictions::new("child"),
            &ctx,
            None,
            &anchor
        )
        .unwrap_err(),
        TokenError::Revoked
    );
}

#[test]
fn a_stale_snapshot_fails_closed() {
    // An expired snapshot is not "fresh" — deny rather than assume validity.
    let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
    let t = token(&anchor);
    let s = RevocationSnapshot::new(
        "old",
        (now() - Duration::hours(2)).to_rfc3339(),
        (now() - Duration::hours(1)).to_rfc3339(), // already expired at `now`
    );
    let signed = fabric_revocation::sign(s, &anchor).unwrap();
    assert_eq!(
        verify_in_context(&t, &ctx_with(anchor.public_key(), &signed)).unwrap_err(),
        TokenError::RevocationUnknown
    );
}
