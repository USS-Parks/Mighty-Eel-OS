//! fabric-revocation tests (Prompt F8): sign/verify, offline queries, tamper
//! rejection, emergency flag, wrong-key rejection; plus the R1 monotonic
//! anti-rollback store.

use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::{
    MonotonicRevocationStore, RevocationError, RevocationSnapshot, sign, verify,
};

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

    // R1: replaying the older, "cleaner" snapshot is refused — the revocation
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
    // Compatibility: a snapshot signed before the R1 `sequence` field existed
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
