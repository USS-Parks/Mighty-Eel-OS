//! fabric-proof tests (Prompt F1): canonical-JSON byte-parity with
//! mai-compliance, subject-hash spec, chain tamper-evidence, and a full
//! sign-with-fabric-crypto → verify-with-fabric-proof bundle round-trip.

use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_proof::{
    AcceptAllBundleVerifier, BundleVerifier, ChainLink, GENESIS_HASH, MlDsaBundleVerifier,
    ProofError, canonical_bytes, chain_link, combined_hash, hmac_subject, verify_chain,
    write_canonical,
};

#[test]
fn canonical_json_matches_mai_compliance_byte_for_byte() {
    // Identical assertion to mai-compliance bundle.rs `canonical_json_is_key_ordered`.
    let a: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1,"c":3}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"c":3,"a":1,"b":2}"#).unwrap();
    let mut buf_a = Vec::new();
    let mut buf_b = Vec::new();
    write_canonical(&mut buf_a, &a);
    write_canonical(&mut buf_b, &b);
    assert_eq!(buf_a, buf_b);
    assert_eq!(buf_a, br#"{"a":1,"b":2,"c":3}"#);
}

#[test]
fn subject_hash_is_deterministic_and_tenant_scoped() {
    let key = vec![7u8; 32];
    let h = hmac_subject(&key, "user-42").unwrap();
    assert!(h.starts_with("hmac:"));
    assert_eq!(h.strip_prefix("hmac:").unwrap().len(), 64);
    assert_eq!(hmac_subject(&key, "user-42").unwrap(), h);
    assert_ne!(hmac_subject(&key, "user-43").unwrap(), h);
    assert_ne!(hmac_subject(&[9u8; 32], "user-42").unwrap(), h); // different key
    assert!(matches!(
        hmac_subject(&[0u8; 16], "x"),
        Err(ProofError::TenantKeyTooShort { got: 16, .. })
    ));
}

#[test]
fn bundle_signed_by_fabric_crypto_verifies_and_rejects_tampering() {
    let signer = RustCryptoMlDsa87::generate("tb-2026-q3").unwrap();
    let payload = serde_json::json!({"metadata": {"tenant_id": "baap"}, "payload": {"x": 1}});
    let hash: [u8; 32] = *blake3::hash(&canonical_bytes(&payload).unwrap()).as_bytes();
    let sig = signer.sign(&hash).unwrap();

    let verifier =
        MlDsaBundleVerifier::new().with_anchor("tb-2026-q3", signer.public_key().to_vec());
    assert_eq!(verifier.anchor_count(), 1);
    assert!(verifier.verify(&hash, &sig, "tb-2026-q3").is_ok());

    let mut tampered = hash;
    tampered[0] ^= 0x01;
    assert!(matches!(
        verifier.verify(&tampered, &sig, "tb-2026-q3"),
        Err(ProofError::InvalidSignature)
    ));
    assert!(matches!(
        verifier.verify(&hash, &sig, "unknown"),
        Err(ProofError::MissingTrustAnchor(_))
    ));
    assert!(
        AcceptAllBundleVerifier
            .verify(&hash, &sig, "anything")
            .is_ok()
    );
}

#[test]
fn combined_hash_matches_metadata_payload_construction() {
    let md = serde_json::json!({"tenant_id": "baap", "version": "v1"});
    let pl = serde_json::json!({"claim_id": "c1"});
    let h1 = combined_hash(&md, &pl).unwrap();
    let combined = serde_json::json!({"metadata": md, "payload": pl});
    let h2: [u8; 32] = *blake3::hash(&canonical_bytes(&combined).unwrap()).as_bytes();
    assert_eq!(h1, h2);
}

#[test]
fn chain_links_and_detects_a_break() {
    let e1 = [1u8; 32];
    let e2 = [2u8; 32];
    let h1 = chain_link(&GENESIS_HASH, &e1);
    let l1 = ChainLink {
        previous_hash: GENESIS_HASH,
        entry_hash: e1,
    };
    let l2 = ChainLink {
        previous_hash: h1,
        entry_hash: e2,
    };
    let final_hash = verify_chain(&[l1, l2]).unwrap();
    assert_eq!(final_hash, chain_link(&h1, &e2));

    let broken = ChainLink {
        previous_hash: [9u8; 32],
        entry_hash: e2,
    };
    assert!(matches!(
        verify_chain(&[l1, broken]),
        Err(ProofError::ChainBroken { index: 1, .. })
    ));
}
