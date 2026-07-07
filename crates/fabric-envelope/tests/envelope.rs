//! fabric-envelope tests (Prompts F4–F6): seal/unseal AEAD round-trip with
//! label-AAD binding, full envelope seal/open, label-readable-without-unseal,
//! and thread tamper-evidence (label, ciphertext, wrong key all break it).

use fabric_contracts::{Classification, ComplianceScope, Label, Route};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_envelope::{
    EnvelopeBinding, EnvelopeError, ThreadSpec, open_envelope, read_label, seal, seal_envelope,
    unseal, verify_thread,
};

fn label() -> Label {
    Label {
        classification: Classification::Restricted,
        compliance_scopes: vec![ComplianceScope::Hipaa],
        origin: "svc:aeneas-gateway".into(),
        permitted_ops: vec!["unseal_local".into()],
        permitted_destinations: vec![Route::LocalOnly],
        detected_entities: vec!["phi.mrn".into()],
    }
}

fn spec() -> ThreadSpec {
    ThreadSpec {
        authorizing_token_id: "tok_1".into(),
        previous_hash: "blake3:0000".into(),
        created_at: "2026-07-03T18:00:00Z".into(),
        binding: EnvelopeBinding {
            tenant_id: "baap".into(),
            owner_subject_hash: "hmac:owner".into(),
            audience: "wsf".into(),
        },
    }
}

#[test]
fn seal_unseal_round_trip() {
    let key = [4u8; 32];
    let aad = b"label-canonical-bytes";
    let s = seal(b"secret PHI", &key, "local:test", aad).unwrap();
    assert_eq!(s.aead_alg, "AES-256-GCM");
    assert_eq!(unseal(&s, &key, aad).unwrap(), b"secret PHI");
    // Wrong key -> authentication failure.
    assert_eq!(
        unseal(&s, &[9u8; 32], aad),
        Err(EnvelopeError::UnsealFailed)
    );
    // Altered label (different AAD) -> caught before decrypt.
    assert_eq!(
        unseal(&s, &key, b"different-aad"),
        Err(EnvelopeError::AadMismatch)
    );
}

#[test]
fn envelope_seals_opens_and_label_is_readable_without_the_key() {
    let signer = RustCryptoMlDsa87::generate("seal-q3").unwrap();
    let key = [7u8; 32];
    let env = seal_envelope(
        "env_1",
        b"regulated data",
        &key,
        "openbao:transit:keys/tenant-baap:v3",
        label(),
        spec(),
        &signer,
    )
    .unwrap();

    // The label reads without any data key — the AOG DSPM-routing hook.
    assert_eq!(read_label(&env).classification, Classification::Restricted);
    assert_eq!(
        read_label(&env).permitted_destinations,
        vec![Route::LocalOnly]
    );

    verify_thread(&env, &MlDsa87Verifier, signer.public_key()).unwrap();
    let pt = open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()).unwrap();
    assert_eq!(pt, b"regulated data");
}

#[test]
fn tampering_the_label_breaks_the_thread() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let key = [7u8; 32];
    let mut env = seal_envelope("env_1", b"data", &key, "x", label(), spec(), &signer).unwrap();
    env.label.classification = Classification::Public; // downgrade after signing
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}

#[test]
fn tampering_the_ciphertext_breaks_the_thread() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let key = [7u8; 32];
    let mut env = seal_envelope("env_1", b"data", &key, "x", label(), spec(), &signer).unwrap();
    env.seal.ciphertext = hex::encode([0xAAu8; 40]); // swap the ciphertext
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}

#[test]
fn tampering_the_tenant_binding_breaks_the_thread() {
    // The tenant/owner/audience binding is signed into the thread (and folded into
    // the AAD): rebinding an envelope to another tenant after sealing breaks it.
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let key = [7u8; 32];
    let mut env = seal_envelope("env_1", b"data", &key, "x", label(), spec(), &signer).unwrap();
    env.thread.tenant_id = "attacker-tenant".into();
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}

#[test]
fn opening_with_the_wrong_public_key_fails() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let impostor = RustCryptoMlDsa87::generate("other").unwrap();
    let key = [7u8; 32];
    let env = seal_envelope("env_1", b"data", &key, "x", label(), spec(), &signer).unwrap();
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, impostor.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}
