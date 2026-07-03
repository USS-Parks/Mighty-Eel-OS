//! fabric-identity tests (Prompt F2): mint/verify round-trip, tamper rejection,
//! session/task child derivation + binding, invalid child kind, pseudonymization.

use fabric_contracts::{Identity, IdentityKind, Signature};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_identity::{ChildSpec, IdentityError, derive_child, mint, pseudonymize, verify};

fn base_identity() -> Identity {
    Identity {
        identity_id: "id_root".into(),
        kind: IdentityKind::Workload,
        tenant_id: "baap".into(),
        subject_id: "svc:aog-gateway".into(),
        subject_hash: "hmac:abc".into(),
        service_identity: Some("aog-gateway".into()),
        spiffe_id: "spiffe://im/t/baap/aog".into(),
        pki_cert_fingerprint: "sha256:x".into(),
        parent_id: None,
        issued_at: "2026-07-03T18:00:00Z".into(),
        expires_at: "2026-07-03T18:15:00Z".into(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    }
}

#[test]
fn mint_then_verify_round_trip() {
    let signer = RustCryptoMlDsa87::generate("bridge-q3").unwrap();
    let signed = mint(base_identity(), &signer).unwrap();
    assert_eq!(signed.signature.alg, "ml-dsa-87");
    verify(&signed, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn tampered_identity_fails() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let mut signed = mint(base_identity(), &signer).unwrap();
    signed.tenant_id = "evil".into();
    assert_eq!(
        verify(&signed, &MlDsa87Verifier, signer.public_key()),
        Err(IdentityError::InvalidSignature)
    );
}

#[test]
fn derive_session_child_binds_parent_and_verifies() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = mint(base_identity(), &signer).unwrap();
    let child = derive_child(
        &parent,
        ChildSpec {
            identity_id: "id_session_1".into(),
            kind: IdentityKind::Session,
            spiffe_id: "spiffe://im/t/baap/aog/session/1".into(),
            issued_at: "2026-07-03T18:01:00Z".into(),
            expires_at: "2026-07-03T18:06:00Z".into(),
        },
        &signer,
    )
    .unwrap();
    assert_eq!(child.parent_id.as_deref(), Some("id_root"));
    assert_eq!(child.kind, IdentityKind::Session);
    assert_eq!(child.tenant_id, "baap"); // inherited
    assert_eq!(child.subject_hash, "hmac:abc"); // inherited
    verify(&child, &MlDsa87Verifier, signer.public_key()).unwrap();
}

#[test]
fn derive_child_rejects_non_session_task_kind() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let parent = mint(base_identity(), &signer).unwrap();
    let err = derive_child(
        &parent,
        ChildSpec {
            identity_id: "id_x".into(),
            kind: IdentityKind::Workload, // not allowed as a child
            spiffe_id: "spiffe://x".into(),
            issued_at: "2026-07-03T18:01:00Z".into(),
            expires_at: "2026-07-03T18:06:00Z".into(),
        },
        &signer,
    )
    .unwrap_err();
    assert_eq!(err, IdentityError::InvalidChildKind(IdentityKind::Workload));
}

#[test]
fn pseudonymize_is_deterministic_and_guards_key_length() {
    let key = vec![3u8; 32];
    let a = pseudonymize(&key, "patient-9").unwrap();
    assert!(a.starts_with("hmac:"));
    assert_eq!(pseudonymize(&key, "patient-9").unwrap(), a);
    assert_ne!(pseudonymize(&key, "patient-8").unwrap(), a);
    assert!(matches!(
        pseudonymize(&[0u8; 16], "x"),
        Err(IdentityError::TenantKeyTooShort { got: 16, .. })
    ));
}
