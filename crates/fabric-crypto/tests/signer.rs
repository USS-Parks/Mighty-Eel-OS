//! fabric-crypto provider tests (Prompt 0.2a): the signer abstraction over
//! pure-Rust ML-DSA-87, plus the fail-closed Transit custody seam.

use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87, TransitSigner};
use fabric_crypto::{CryptoError, MLDSA87_PK_LEN, MLDSA87_SIG_LEN, Signer, Verifier};

#[test]
fn rustcrypto_sign_verify_round_trip() {
    let signer = RustCryptoMlDsa87::generate("bridge-2026-q3").unwrap();
    assert_eq!(signer.algorithm(), "ml-dsa-87");
    assert_eq!(signer.key_id(), "bridge-2026-q3");
    assert_eq!(signer.public_key().len(), MLDSA87_PK_LEN);

    let msg = b"canonical bytes of a trust bundle";
    let sig = signer.sign(msg).unwrap();
    assert_eq!(sig.len(), MLDSA87_SIG_LEN);

    let verifier = MlDsa87Verifier;
    assert!(verifier.verify(msg, &sig, signer.public_key()).unwrap());
}

#[test]
fn tamper_and_wrong_key_fail_closed() {
    let signer = RustCryptoMlDsa87::generate("k1").unwrap();
    let other = RustCryptoMlDsa87::generate("k2").unwrap();
    let msg = b"original message";
    let sig = signer.sign(msg).unwrap();
    let verifier = MlDsa87Verifier;

    assert!(
        !verifier
            .verify(b"tampered message", &sig, signer.public_key())
            .unwrap()
    );
    assert!(!verifier.verify(msg, &sig, other.public_key()).unwrap());
    // Wrong-sized inputs return Ok(false), not an error.
    assert!(
        !verifier
            .verify(msg, &[0u8; 10], signer.public_key())
            .unwrap()
    );
}

#[test]
fn from_keypair_reconstructs_a_usable_signer() {
    let (pk, sk) = RustCryptoMlDsa87::keypair().unwrap();
    let signer = RustCryptoMlDsa87::from_keypair("stored", pk.clone(), sk).unwrap();
    let msg = b"reconstructed-signer message";
    let sig = signer.sign(msg).unwrap();
    assert!(MlDsa87Verifier.verify(msg, &sig, &pk).unwrap());

    // Malformed key material is rejected.
    assert!(RustCryptoMlDsa87::from_keypair("bad", vec![0u8; 10], vec![0u8; 10]).is_err());
}

#[test]
fn signer_drop_is_sound_after_zeroize_wiring() {
    // The signer wipes its secret key on drop. Exercise the Drop path and
    // confirm an independent signer is unaffected (no double-free, no cross-talk).
    let msg = b"post-drop soundness";
    let survivor = RustCryptoMlDsa87::generate("survivor").unwrap();
    let survivor_sig = survivor.sign(msg).unwrap();
    {
        let ephemeral = RustCryptoMlDsa87::generate("ephemeral").unwrap();
        let _ = ephemeral.sign(msg).unwrap();
    } // `ephemeral` drops here -> its secret_key is zeroized.
    assert!(
        MlDsa87Verifier
            .verify(msg, &survivor_sig, survivor.public_key())
            .unwrap()
    );
}

#[test]
fn transit_signer_is_the_seam_and_fails_closed() {
    let transit = TransitSigner::new("tenant-baap-bundle");
    assert_eq!(transit.algorithm(), "ml-dsa-87");
    assert_eq!(transit.key_id(), "tenant-baap-bundle");
    assert!(matches!(
        transit.sign(b"x"),
        Err(CryptoError::Unavailable(_))
    ));
}
