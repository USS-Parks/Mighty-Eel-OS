//! fabric-envelope tests (Prompts F4–F6): seal/unseal AEAD round-trip with
//! label-AAD binding, full envelope seal/open, label-readable-without-unseal,
//! and thread tamper-evidence (label, ciphertext, wrong key all break it).

use fabric_contracts::{
    Classification, ComplianceScope, Envelope, EnvelopeBinding, Label, Route, Signature, Thread,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_envelope::{
    EnvelopeError, ThreadSpec, migrate_legacy, open_envelope, read_label, seal, seal_envelope,
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

fn binding() -> EnvelopeBinding {
    EnvelopeBinding {
        tenant_id: "baap".into(),
        owner_subject_hash: "hmac:abc".into(),
        audience: "wsf".into(),
        envelope_version: 2,
    }
}

fn spec() -> ThreadSpec {
    ThreadSpec {
        authorizing_token_id: "tok_1".into(),
        previous_hash: "blake3:0000".into(),
        created_at: "2026-07-03T18:00:00Z".into(),
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
        binding(),
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
fn tampering_the_binding_breaks_the_envelope() {
    // E1: the tenant/owner binding is authenticated. Swapping the tenant after
    // sealing breaks both the provenance thread and the AEAD AAD.
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let key = [7u8; 32];
    let mut env = seal_envelope(
        "env_1",
        b"data",
        &key,
        "x",
        label(),
        binding(),
        spec(),
        &signer,
    )
    .unwrap();
    env.binding.tenant_id = "attacker-tenant".into(); // cross-tenant swap
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}

#[test]
fn tampering_the_label_breaks_the_thread() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let key = [7u8; 32];
    let mut env = seal_envelope(
        "env_1",
        b"data",
        &key,
        "x",
        label(),
        binding(),
        spec(),
        &signer,
    )
    .unwrap();
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
    let mut env = seal_envelope(
        "env_1",
        b"data",
        &key,
        "x",
        label(),
        binding(),
        spec(),
        &signer,
    )
    .unwrap();
    env.seal.ciphertext = hex::encode([0xAAu8; 40]); // swap the ciphertext
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, signer.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}

/// Build a legacy (v1) envelope by hand: AAD = label only, thread content with
/// no `binding` key, empty binding — the pre-E1 format.
fn seal_v1(signer: &RustCryptoMlDsa87, key: &[u8; 32], plaintext: &[u8]) -> Envelope {
    let label = label();
    let aad = fabric_proof::canonical_bytes(&label).unwrap();
    let seal = seal(plaintext, key, "legacy:wrap", &aad).unwrap();
    let content = serde_json::json!({
        "envelope_id": "v1-env",
        "seal": seal,
        "label": label,
        "authorizing_token_id": "tok_1",
        "previous_hash": "blake3:0000",
        "created_at": "2025-01-01T00:00:00Z",
    });
    let hash = fabric_proof::canonical_hash(&content).unwrap();
    let sig = signer.sign(&hash).unwrap();
    Envelope {
        envelope_id: "v1-env".into(),
        seal,
        label,
        binding: EnvelopeBinding::default(), // unbound v1
        thread: Thread {
            created_at: "2025-01-01T00:00:00Z".into(),
            authorizing_token_id: "tok_1".into(),
            previous_hash: "blake3:0000".into(),
            signatures: vec![Signature {
                alg: signer.algorithm().to_string(),
                key_id: signer.key_id().to_string(),
                value: hex::encode(sig),
            }],
        },
    }
}

#[test]
fn legacy_v1_envelope_migrates_to_v2_and_is_idempotent() {
    let signer = RustCryptoMlDsa87::generate("seal-svc").unwrap();
    let key = [7u8; 32];
    let v1 = seal_v1(&signer, &key, b"legacy regulated data");

    // A v1 envelope cannot be opened by the v2 path (its AAD/thread differ).
    assert!(open_envelope(&v1, &key, &MlDsa87Verifier, signer.public_key()).is_err());

    // Migrate it to v2 with a tenant binding, authenticated against the original
    // sealer key.
    let v2 = migrate_legacy(
        &v1,
        &key,
        binding(),
        &MlDsa87Verifier,
        signer.public_key(),
        &signer,
    )
    .unwrap();
    assert_eq!(v2.binding.envelope_version, 2);
    assert_eq!(v2.binding.tenant_id, "baap");

    // The migrated v2 envelope opens and round-trips the payload.
    let pt = open_envelope(&v2, &key, &MlDsa87Verifier, signer.public_key()).unwrap();
    assert_eq!(pt, b"legacy regulated data");

    // Idempotent: migrating an already-v2 envelope returns it unchanged.
    let again = migrate_legacy(
        &v2,
        &key,
        binding(),
        &MlDsa87Verifier,
        signer.public_key(),
        &signer,
    )
    .unwrap();
    assert_eq!(again.binding.envelope_version, 2);
    assert_eq!(again.thread.signatures, v2.thread.signatures);
}

#[test]
fn migrating_a_tampered_v1_envelope_is_rejected() {
    let signer = RustCryptoMlDsa87::generate("seal-svc").unwrap();
    let key = [7u8; 32];
    let mut v1 = seal_v1(&signer, &key, b"data");
    v1.label.classification = Classification::Public; // tamper after v1 sign
    assert_eq!(
        migrate_legacy(
            &v1,
            &key,
            binding(),
            &MlDsa87Verifier,
            signer.public_key(),
            &signer
        ),
        Err(EnvelopeError::InvalidSignature)
    );
}

#[test]
fn opening_with_the_wrong_public_key_fails() {
    let signer = RustCryptoMlDsa87::generate("k").unwrap();
    let impostor = RustCryptoMlDsa87::generate("other").unwrap();
    let key = [7u8; 32];
    let env = seal_envelope(
        "env_1",
        b"data",
        &key,
        "x",
        label(),
        binding(),
        spec(),
        &signer,
    )
    .unwrap();
    assert_eq!(
        open_envelope(&env, &key, &MlDsa87Verifier, impostor.public_key()),
        Err(EnvelopeError::InvalidSignature)
    );
}
