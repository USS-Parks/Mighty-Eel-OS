//! fabric-revocation tests (Prompt F8): sign/verify, offline queries, tamper
//! rejection, emergency flag, wrong-key rejection.

use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::{RevocationError, RevocationSnapshot, sign, verify};

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
