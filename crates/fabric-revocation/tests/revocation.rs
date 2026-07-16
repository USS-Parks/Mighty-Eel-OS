//! fabric-revocation tests (Prompt F8): sign/verify, offline queries, tamper
//! rejection, emergency flag, wrong-key rejection; plus the monotonic
//! anti-rollback store.

use chrono::{DateTime, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::{
    CurrentRevocationError, MonotonicRevocationStore, RevocationError, RevocationSnapshot, sign,
    verify,
};

fn token() -> TrustToken {
    TrustToken {
        token_id: "tok-a".into(),
        issued_at: "2026-07-15T00:00:00Z".into(),
        expires_at: "2026-07-16T00:00:00Z".into(),
        issuer: "issuer-a".into(),
        trust_bundle_version: "bundle-a".into(),
        tenant_id: "tenant-a".into(),
        subject_id: None,
        subject_hash: "subject-a".into(),
        service_identity: Some("service-a".into()),
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: "ML-DSA-87".into(),
            key_id: "key-a".into(),
            value: String::new(),
        },
    }
}

fn trusted_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn sign_verify_and_query() {
    let signer = RustCryptoMlDsa87::generate("rev-q3").unwrap();
    let mut snap = RevocationSnapshot::new("rev_1", "2026-07-03T18:00:00Z", "2026-07-03T19:00:00Z");
    snap.revoked_tokens.push("tok_bad".into());
    snap.revoked_subjects.push("hmac:evil".into());
    snap.revoked_bundle_versions.push("2026.05.20.003".into());

    let signed = sign(snap, &signer).unwrap();
    verify(&signed, &MlDsa87Verifier, signer.public_key()).unwrap();
    assert!(signed.is_token_revoked("tok_bad"));
    assert!(!signed.is_token_revoked("tok_ok"));
    assert!(signed.is_subject_revoked("hmac:evil"));
    assert!(signed.is_bundle_revoked("2026.05.20.003"));
    assert!(!signed.emergency);
}

#[test]
fn tampering_after_signing_fails() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let signed = sign(RevocationSnapshot::new("rev_1", "a", "b"), &signer).unwrap();
    let mut tampered = signed.clone();
    tampered.revoked_tokens.push("tok_sneaky".into()); // revoke more after signing
    assert_eq!(
        verify(&tampered, &MlDsa87Verifier, signer.public_key()),
        Err(RevocationError::InvalidSignature)
    );
}

#[test]
fn emergency_snapshot_flag_round_trips() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let signed = sign(
        RevocationSnapshot::new("rev_emg", "a", "b").emergency(),
        &signer,
    )
    .unwrap();
    assert!(signed.emergency);
    verify(&signed, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn wrong_key_fails() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let impostor = RustCryptoMlDsa87::generate("other").unwrap();
    let signed = sign(RevocationSnapshot::new("r", "a", "b"), &signer).unwrap();
    assert_eq!(
        verify(&signed, &MlDsa87Verifier, impostor.public_key()),
        Err(RevocationError::InvalidSignature)
    );
}

#[test]
fn store_advances_only_on_strictly_newer_verified_snapshots() {
    let signer = RustCryptoMlDsa87::generate("rev-anchor").unwrap();
    let mut store = MonotonicRevocationStore::new();
    assert!(store.current().is_none(), "empty store holds nothing");

    let s1 = sign(
        RevocationSnapshot::new("rev_1", "a", "b").with_sequence(1),
        &signer,
    )
    .unwrap();
    assert_eq!(
        store
            .advance(s1, &MlDsa87Verifier, signer.public_key())
            .unwrap(),
        1
    );

    let mut newer = RevocationSnapshot::new("rev_2", "a", "b").with_sequence(2);
    newer.revoked_tenants.push("tenant-evil".into());
    let s2 = sign(newer, &signer).unwrap();
    assert_eq!(
        store
            .advance(s2, &MlDsa87Verifier, signer.public_key())
            .unwrap(),
        2
    );
    assert!(
        store
            .current()
            .is_some_and(|s| s.is_tenant_revoked("tenant-evil"))
    );

    // Replaying the older, "cleaner" snapshot is refused — the revocation
    // of tenant-evil cannot be rolled back.
    let stale = sign(
        RevocationSnapshot::new("rev_1", "a", "b").with_sequence(1),
        &signer,
    )
    .unwrap();
    assert_eq!(
        store.advance(stale, &MlDsa87Verifier, signer.public_key()),
        Err(RevocationError::Rollback {
            current: 2,
            candidate: 1
        })
    );
    // Equal sequence is also a refused replay.
    let equal = sign(
        RevocationSnapshot::new("rev_2b", "a", "b").with_sequence(2),
        &signer,
    )
    .unwrap();
    assert!(matches!(
        store.advance(equal, &MlDsa87Verifier, signer.public_key()),
        Err(RevocationError::Rollback { .. })
    ));
    assert!(
        store
            .current()
            .is_some_and(|s| s.is_tenant_revoked("tenant-evil")),
        "held state survives rollback attempts"
    );
}

#[test]
fn store_refuses_unverified_snapshots() {
    let signer = RustCryptoMlDsa87::generate("rev-anchor").unwrap();
    let impostor = RustCryptoMlDsa87::generate("impostor").unwrap();
    let mut store = MonotonicRevocationStore::new();

    // Signed by the wrong key → never enters the store, even at sequence 1.
    let forged = sign(
        RevocationSnapshot::new("rev_f", "a", "b").with_sequence(1),
        &impostor,
    )
    .unwrap();
    assert_eq!(
        store.advance(forged, &MlDsa87Verifier, signer.public_key()),
        Err(RevocationError::InvalidSignature)
    );
    assert!(store.current().is_none());
}

#[test]
fn pre_sequence_snapshots_still_verify() {
    // Compatibility: a snapshot signed before the `sequence` field existed
    // serializes identically at sequence 0, so its signature still verifies.
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let signed = sign(RevocationSnapshot::new("rev_old", "a", "b"), &signer).unwrap();
    assert_eq!(signed.sequence, 0);
    let json = serde_json::to_string(&signed).unwrap();
    assert!(
        !json.contains("sequence"),
        "zero sequence is not serialized"
    );
    let back: RevocationSnapshot = serde_json::from_str(&json).unwrap();
    verify(&back, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn current_consumer_contract_fails_closed_for_absent_invalid_stale_and_revoked_state() {
    let anchor = RustCryptoMlDsa87::generate("rev-anchor-current").unwrap();
    let impostor = RustCryptoMlDsa87::generate("rev-impostor-current").unwrap();
    let tok = token();

    let store = MonotonicRevocationStore::new();
    assert_eq!(
        store.authorize(&tok, trusted_now()),
        Err(CurrentRevocationError::Unavailable)
    );
    assert_eq!(
        store.ensure_current(trusted_now()),
        Err(CurrentRevocationError::Unavailable)
    );

    let mut store = MonotonicRevocationStore::new();
    let forged = sign(
        RevocationSnapshot::new("forged", "2026-07-15T00:00:00Z", "2026-07-16T00:00:00Z")
            .with_sequence(1),
        &impostor,
    )
    .unwrap();
    assert_eq!(
        store.advance(forged, &MlDsa87Verifier, anchor.public_key()),
        Err(RevocationError::InvalidSignature)
    );
    assert_eq!(
        store.authorize(&tok, trusted_now()),
        Err(CurrentRevocationError::Unavailable)
    );

    for (label, issued_at, expires_at, expected) in [
        (
            "unsequenced",
            "2026-07-15T00:00:00Z",
            "2026-07-16T00:00:00Z",
            CurrentRevocationError::Unsequenced,
        ),
        (
            "future",
            "2026-07-15T13:00:00Z",
            "2026-07-16T00:00:00Z",
            CurrentRevocationError::NotYetValid,
        ),
        (
            "stale",
            "2026-07-14T00:00:00Z",
            "2026-07-15T12:00:00Z",
            CurrentRevocationError::Expired,
        ),
    ] {
        let sequence = u64::from(label != "unsequenced");
        let snapshot = sign(
            RevocationSnapshot::new(label, issued_at, expires_at).with_sequence(sequence),
            &anchor,
        )
        .unwrap();
        let mut candidate = MonotonicRevocationStore::new();
        candidate
            .advance(snapshot, &MlDsa87Verifier, anchor.public_key())
            .unwrap();
        assert_eq!(
            candidate.authorize(&tok, trusted_now()),
            Err(expected),
            "{label}"
        );
    }

    let mut revoked =
        RevocationSnapshot::new("revoked", "2026-07-15T00:00:00Z", "2026-07-16T00:00:00Z")
            .with_sequence(2);
    revoked.revoked_tenants.push("tenant-a".into());
    let mut store = MonotonicRevocationStore::new();
    store
        .advance(
            sign(revoked, &anchor).unwrap(),
            &MlDsa87Verifier,
            anchor.public_key(),
        )
        .unwrap();
    assert_eq!(
        store.authorize(&tok, trusted_now()),
        Err(CurrentRevocationError::Revoked("tenant"))
    );
    assert_eq!(store.ensure_current(trusted_now()), Ok(2));

    let cleaner_rollback = sign(
        RevocationSnapshot::new(
            "cleaner-rollback",
            "2026-07-15T00:00:00Z",
            "2026-07-16T00:00:00Z",
        )
        .with_sequence(1),
        &anchor,
    )
    .unwrap();
    assert!(matches!(
        store.advance(cleaner_rollback, &MlDsa87Verifier, anchor.public_key()),
        Err(RevocationError::Rollback { .. })
    ));
    assert_eq!(
        store.authorize(&tok, trusted_now()),
        Err(CurrentRevocationError::Revoked("tenant")),
        "a rejected lower sequence cannot replace held revocations"
    );
}
