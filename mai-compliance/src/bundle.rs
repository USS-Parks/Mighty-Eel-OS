//! Signed claim and policy-bundle verification.
//!
//! See `docs/compliance/TRUST-BUNDLE-SPEC.md` for the wire format and verification
//! algorithm. This module owns the Rust projection of those schemas plus
//! the [`BundleVerifier`] trait and an ML-DSA-87-backed default impl.
//!
//! Two top-level envelopes:
//!
//! - [`SignedPolicyBundle`] — the Trust Bridge's periodic revocation
//!   snapshot. Consumed by [`crate::trust_cache::LocalTrustCache::record_signed_refresh`].
//! - [`SignedClaim`] — a per-subject assertion. Consumed at request time
//!   when the policy runtime needs to validate a caller's
//!   trust context.
//!
//! Verification is canonical-JSON over `{metadata, payload}`, BLAKE3 to a
//! 32-byte digest, then ML-DSA-87 signature verification against a
//! registered trust anchor.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::trust_cache::RevocationSnapshot;

/// ML-DSA-87 public-key length in bytes (FIPS 204).
const MLDSA87_PK_LEN: usize = 2592;
/// ML-DSA-87 signature length in bytes (FIPS 204).
const MLDSA87_SIG_LEN: usize = 4627;

// ---------------------------------------------------------------------------
// Wire schemas
// ---------------------------------------------------------------------------

/// Metadata header shared by [`SignedPolicyBundle`] and [`SignedClaim`].
///
/// All timestamps are Unix epoch seconds. The wire format uses unsigned
/// integers; receivers reject negative values at decode time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMetadata {
    /// Bundle / claim version identifier (e.g. `"2026.05.22.001"`).
    /// Surfaces in audit records as `trust_bundle_version`.
    pub version: String,
    /// Identifier of the signing service (e.g. `"trust-bridge"`).
    pub issuer: String,
    /// Wall-clock time the bundle was signed, Unix epoch seconds.
    pub issued_at_secs: u64,
    /// Wall-clock time the bundle expires, Unix epoch seconds.
    pub expires_at_secs: u64,
    /// Tenant the bundle is scoped to. Cross-tenant bundles are rejected
    /// at the trust-cache refresh boundary.
    pub tenant_id: String,
}

/// Signature envelope carried alongside the payload on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEnvelope {
    /// Signature algorithm identifier. Only `"ml-dsa-87"` is accepted
    /// in this build; future algorithms surface here.
    pub algorithm: String,
    /// Identifier of the public key that should be used to verify.
    /// Resolved against the local [`MlDsaBundleVerifier`] anchor registry.
    pub public_key_id: String,
    /// Lowercase-hex-encoded signature bytes.
    pub bytes_hex: String,
}

/// Body of a [`SignedPolicyBundle`] — one revocation snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBundlePayload {
    /// Per-claim revocation entries. Order is preserved on the wire but
    /// canonical-JSON encoding sorts by `claim_id` before hashing.
    pub revocations: Vec<RevocationSnapshot>,
}

/// Body of a [`SignedClaim`] — one subject-level trust assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimPayload {
    /// Stable claim id used for revocation lookup.
    pub claim_id: String,
    /// HMAC-hashed subject identifier (see [`crate::subject_hash`]).
    /// Always begins with the `"hmac:"` prefix.
    pub subject_hash: String,
    /// Service identity the claim was issued for.
    pub service_identity: String,
    /// Compliance scopes (e.g. `["hipaa", "ocap"]`).
    pub compliance_scopes: Vec<String>,
    /// Allowed routes (e.g. `["local-only"]`).
    pub allowed_routes: Vec<String>,
    /// Data classification (e.g. `"phi"`).
    pub data_classification: String,
}

/// Trust Bridge revocation bundle on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPolicyBundle {
    pub metadata: BundleMetadata,
    pub payload: PolicyBundlePayload,
    pub signature: SignatureEnvelope,
}

/// Per-subject trust assertion on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedClaim {
    pub metadata: BundleMetadata,
    pub payload: ClaimPayload,
    pub signature: SignatureEnvelope,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced when verifying a [`SignedPolicyBundle`] or [`SignedClaim`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BundleError {
    /// Bundle's `expires_at_secs` is at or before `now_secs`.
    #[error("bundle expired at {expires_at_secs} (now {now_secs})")]
    Expired { expires_at_secs: u64, now_secs: u64 },
    /// Bundle's `issued_at_secs` is after `now_secs` — clock skew or
    /// pre-dated bundle.
    #[error("bundle not yet valid: issued at {issued_at_secs} (now {now_secs})")]
    NotYetValid { issued_at_secs: u64, now_secs: u64 },
    /// `signature.algorithm` is not a value this build accepts.
    #[error("unsupported signature algorithm: {0}")]
    UnsupportedAlgorithm(String),
    /// `signature.public_key_id` is not in the local anchor registry.
    #[error("no trust anchor registered for public_key_id {0:?}")]
    MissingTrustAnchor(String),
    /// The hex-decoded signature is the wrong size, malformed, or fails
    /// ML-DSA-87 verification against the claimed public key.
    #[error("signature failed verification")]
    InvalidSignature,
    /// The public key bytes in the anchor registry have the wrong size.
    #[error("trust anchor public key is malformed")]
    InvalidPublicKey,
    /// `signature.bytes_hex` is not valid lowercase hex.
    #[error("signature bytes are not valid hex")]
    MalformedSignatureHex,
    /// Canonical-JSON serialization of `{metadata, payload}` failed.
    #[error("payload could not be serialized for hashing: {0}")]
    Serialize(String),
}

// ---------------------------------------------------------------------------
// Canonical-JSON hashing
// ---------------------------------------------------------------------------

/// Canonicalize a `serde_json::Value` into a deterministic byte sequence.
///
/// Object keys are emitted in lexicographic order. Arrays preserve their
/// order. Numbers and strings use the default `serde_json` encoding.
fn write_canonical(out: &mut Vec<u8>, value: &serde_json::Value) {
    // SOV-F1: the canonical-JSON encoding is owned by `fabric-proof` and is
    // byte-identical to the implementation this replaced, so bundles hash the
    // same in mai-compliance and WSF.
    fabric_proof::write_canonical(out, value);
}

/// Compute the 32-byte BLAKE3 hash of the canonical-JSON encoding of
/// `{metadata, payload}`. This is the value that gets signed and
/// verified.
pub fn payload_hash<P: Serialize>(
    metadata: &BundleMetadata,
    payload: &P,
) -> Result<[u8; 32], BundleError> {
    let metadata_value =
        serde_json::to_value(metadata).map_err(|e| BundleError::Serialize(e.to_string()))?;
    let payload_value =
        serde_json::to_value(payload).map_err(|e| BundleError::Serialize(e.to_string()))?;
    let mut combined = BTreeMap::new();
    combined.insert("metadata".to_string(), metadata_value);
    combined.insert("payload".to_string(), payload_value);
    let combined_value =
        serde_json::to_value(combined).map_err(|e| BundleError::Serialize(e.to_string()))?;
    let mut buf = Vec::new();
    write_canonical(&mut buf, &combined_value);
    Ok(*blake3::hash(&buf).as_bytes())
}

// ---------------------------------------------------------------------------
// Verifier trait
// ---------------------------------------------------------------------------

/// Abstraction over signature verification so tests and alternate
/// backends can substitute their own implementation.
///
/// Production code uses [`MlDsaBundleVerifier`]; tests can use
/// [`AcceptAllBundleVerifier`] or [`RejectAllBundleVerifier`] to exercise
/// path coverage without needing a real keypair.
pub trait BundleVerifier {
    /// Verify `signature_bytes` against `payload_hash` using the public
    /// key registered under `public_key_id`.
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), BundleError>;
}

/// ML-DSA-87-backed [`BundleVerifier`].
///
/// Holds an in-memory registry mapping `public_key_id -> public_key_bytes`.
/// The registry is populated by the operator at boot from configuration
/// or from the local vault.
#[derive(Debug, Clone, Default)]
pub struct MlDsaBundleVerifier {
    anchors: BTreeMap<String, Vec<u8>>,
}

impl MlDsaBundleVerifier {
    /// Construct an empty verifier. Trust anchors must be added with
    /// [`Self::with_anchor`] before any verification will succeed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a trust anchor public key.
    ///
    /// Returns the verifier so calls can be chained. The public key
    /// length is **not** validated here — verification surfaces an
    /// [`BundleError::InvalidPublicKey`] at use time if the registered
    /// bytes are malformed.
    #[must_use]
    pub fn with_anchor(mut self, public_key_id: impl Into<String>, public_key: Vec<u8>) -> Self {
        self.anchors.insert(public_key_id.into(), public_key);
        self
    }

    /// Number of trust anchors currently registered.
    #[must_use]
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }
}

impl BundleVerifier for MlDsaBundleVerifier {
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), BundleError> {
        use ml_dsa::signature::Verifier;
        use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa87, Signature, VerifyingKey};

        let pk_bytes = self
            .anchors
            .get(public_key_id)
            .ok_or_else(|| BundleError::MissingTrustAnchor(public_key_id.to_string()))?;
        let pk_arr: &[u8; MLDSA87_PK_LEN] = pk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| BundleError::InvalidPublicKey)?;
        if signature_bytes.len() != MLDSA87_SIG_LEN {
            return Err(BundleError::InvalidSignature);
        }
        let sig_arr: &[u8; MLDSA87_SIG_LEN] = signature_bytes
            .try_into()
            .map_err(|_| BundleError::InvalidSignature)?;
        let pk_encoded = EncodedVerifyingKey::<MlDsa87>::from(*pk_arr);
        let pk = VerifyingKey::<MlDsa87>::decode(&pk_encoded);
        let sig_encoded = EncodedSignature::<MlDsa87>::from(*sig_arr);
        let sig =
            Signature::<MlDsa87>::decode(&sig_encoded).ok_or(BundleError::InvalidSignature)?;
        pk.verify(payload_hash, &sig)
            .map_err(|_| BundleError::InvalidSignature)
    }
}

/// Test helper: a verifier that accepts every signature. Never use in
/// production paths.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptAllBundleVerifier;

impl BundleVerifier for AcceptAllBundleVerifier {
    fn verify(
        &self,
        _payload_hash: &[u8; 32],
        _signature_bytes: &[u8],
        _public_key_id: &str,
    ) -> Result<(), BundleError> {
        Ok(())
    }
}

/// Test helper: a verifier that rejects every signature. Never use in
/// production paths.
#[derive(Debug, Default, Clone, Copy)]
pub struct RejectAllBundleVerifier;

impl BundleVerifier for RejectAllBundleVerifier {
    fn verify(
        &self,
        _payload_hash: &[u8; 32],
        _signature_bytes: &[u8],
        _public_key_id: &str,
    ) -> Result<(), BundleError> {
        Err(BundleError::InvalidSignature)
    }
}

// ---------------------------------------------------------------------------
// Envelope verification helpers
// ---------------------------------------------------------------------------

/// Validate the metadata window (`issued_at` <= `now` < `expires_at`)
/// and the signature algorithm string.
fn check_window_and_algorithm(
    metadata: &BundleMetadata,
    signature: &SignatureEnvelope,
    now_secs: u64,
) -> Result<(), BundleError> {
    if signature.algorithm != "ml-dsa-87" {
        return Err(BundleError::UnsupportedAlgorithm(
            signature.algorithm.clone(),
        ));
    }
    if metadata.issued_at_secs > now_secs {
        return Err(BundleError::NotYetValid {
            issued_at_secs: metadata.issued_at_secs,
            now_secs,
        });
    }
    if metadata.expires_at_secs <= now_secs {
        return Err(BundleError::Expired {
            expires_at_secs: metadata.expires_at_secs,
            now_secs,
        });
    }
    Ok(())
}

impl SignedPolicyBundle {
    /// Verify this bundle's metadata window and signature.
    ///
    /// Returns a borrow of the validated payload on success. The cache
    /// caller then chooses how to apply it. On any failure, no state
    /// changes: caller's trust cache is preserved as-is.
    pub fn verified_payload<V: BundleVerifier>(
        &self,
        verifier: &V,
        now_secs: u64,
    ) -> Result<&PolicyBundlePayload, BundleError> {
        check_window_and_algorithm(&self.metadata, &self.signature, now_secs)?;
        let hash = payload_hash(&self.metadata, &self.payload)?;
        let sig_bytes = hex::decode(&self.signature.bytes_hex)
            .map_err(|_| BundleError::MalformedSignatureHex)?;
        verifier.verify(&hash, &sig_bytes, &self.signature.public_key_id)?;
        Ok(&self.payload)
    }
}

impl SignedClaim {
    /// Verify this claim's metadata window and signature.
    pub fn verified_payload<V: BundleVerifier>(
        &self,
        verifier: &V,
        now_secs: u64,
    ) -> Result<&ClaimPayload, BundleError> {
        check_window_and_algorithm(&self.metadata, &self.signature, now_secs)?;
        let hash = payload_hash(&self.metadata, &self.payload)?;
        let sig_bytes = hex::decode(&self.signature.bytes_hex)
            .map_err(|_| BundleError::MalformedSignatureHex)?;
        verifier.verify(&hash, &sig_bytes, &self.signature.public_key_id)?;
        Ok(&self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust_cache::SnapshotStatus;

    /// Sign a payload hash with an ML-DSA-87 secret key. Test-only.
    fn sign_with(secret_key: &[u8], payload_hash: &[u8; 32]) -> Vec<u8> {
        use ml_dsa::signature::Signer;
        use ml_dsa::{EncodedSigningKey, MlDsa87, Signature, SigningKey};
        const SK_LEN: usize = 4896;
        let sk_arr: &[u8; SK_LEN] = secret_key.try_into().unwrap();
        let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(payload_hash);
        sig.encode().to_vec()
    }

    /// Generate a fresh ML-DSA-87 keypair for tests.
    fn fresh_keypair() -> (Vec<u8>, Vec<u8>) {
        use ml_dsa::{B32, KeyGen, MlDsa87};
        use rand::RngCore;
        let mut seed_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed_bytes);
        let seed = B32::from(seed_bytes);
        let kp = MlDsa87::key_gen_internal(&seed);
        let pk = kp.verifying_key().encode().to_vec();
        let sk = kp.signing_key().encode().to_vec();
        (pk, sk)
    }

    fn sample_metadata(issued: u64, expires: u64) -> BundleMetadata {
        BundleMetadata {
            version: "2026.05.22.001".to_string(),
            issuer: "trust-bridge".to_string(),
            issued_at_secs: issued,
            expires_at_secs: expires,
            tenant_id: "tribal-health-demo".to_string(),
        }
    }

    fn sample_payload() -> PolicyBundlePayload {
        PolicyBundlePayload {
            revocations: vec![RevocationSnapshot {
                claim_id: "claim-001".to_string(),
                status: SnapshotStatus::Valid,
                recorded_at_secs: 1_000,
            }],
        }
    }

    fn build_signed_bundle(
        metadata: BundleMetadata,
        payload: PolicyBundlePayload,
        secret_key: &[u8],
        public_key_id: &str,
    ) -> SignedPolicyBundle {
        let hash = payload_hash(&metadata, &payload).unwrap();
        let sig_bytes = sign_with(secret_key, &hash);
        SignedPolicyBundle {
            metadata,
            payload,
            signature: SignatureEnvelope {
                algorithm: "ml-dsa-87".to_string(),
                public_key_id: public_key_id.to_string(),
                bytes_hex: hex::encode(sig_bytes),
            },
        }
    }

    #[test]
    fn canonical_json_is_key_ordered() {
        // Two objects with identical content but different insertion
        // order must produce the same canonical bytes.
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
    fn valid_bundle_verifies() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb-2026-q2", pk);
        let bundle = build_signed_bundle(
            sample_metadata(1_000, 2_000),
            sample_payload(),
            &sk,
            "tb-2026-q2",
        );
        let payload = bundle.verified_payload(&verifier, 1_500).unwrap();
        assert_eq!(payload.revocations.len(), 1);
    }

    #[test]
    fn expired_bundle_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        let err = bundle.verified_payload(&verifier, 2_000).unwrap_err();
        assert!(matches!(err, BundleError::Expired { .. }));
        // 2_001 too.
        let err2 = bundle.verified_payload(&verifier, 2_001).unwrap_err();
        assert!(matches!(err2, BundleError::Expired { .. }));
    }

    #[test]
    fn future_bundle_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        let err = bundle.verified_payload(&verifier, 500).unwrap_err();
        assert!(matches!(err, BundleError::NotYetValid { .. }));
    }

    #[test]
    fn tampered_payload_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let mut bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        // Mutate the payload AFTER signing.
        bundle.payload.revocations[0].status = SnapshotStatus::Revoked;
        let err = bundle.verified_payload(&verifier, 1_500).unwrap_err();
        assert_eq!(err, BundleError::InvalidSignature);
    }

    #[test]
    fn tampered_metadata_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let mut bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        // Tenant swap after signing.
        bundle.metadata.tenant_id = "other-tenant".to_string();
        let err = bundle.verified_payload(&verifier, 1_500).unwrap_err();
        assert_eq!(err, BundleError::InvalidSignature);
    }

    #[test]
    fn unknown_anchor_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let bundle = build_signed_bundle(
            sample_metadata(1_000, 2_000),
            sample_payload(),
            &sk,
            "different-key-id",
        );
        let err = bundle.verified_payload(&verifier, 1_500).unwrap_err();
        assert!(matches!(err, BundleError::MissingTrustAnchor(_)));
    }

    #[test]
    fn unsupported_algorithm_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let mut bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        bundle.signature.algorithm = "rsa-2048".to_string();
        let err = bundle.verified_payload(&verifier, 1_500).unwrap_err();
        assert!(matches!(err, BundleError::UnsupportedAlgorithm(_)));
    }

    #[test]
    fn malformed_hex_rejected() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tb", pk);
        let mut bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        bundle.signature.bytes_hex = "not-hex".to_string();
        let err = bundle.verified_payload(&verifier, 1_500).unwrap_err();
        assert_eq!(err, BundleError::MalformedSignatureHex);
    }

    #[test]
    fn signed_claim_verifies() {
        let (pk, sk) = fresh_keypair();
        let verifier = MlDsaBundleVerifier::new().with_anchor("tenant-key", pk);
        let metadata = sample_metadata(1_000, 2_000);
        let payload = ClaimPayload {
            claim_id: "claim-001".to_string(),
            subject_hash: "hmac:abcd".to_string(),
            service_identity: "lamprey-router".to_string(),
            compliance_scopes: vec!["hipaa".to_string(), "ocap".to_string()],
            allowed_routes: vec!["local-only".to_string()],
            data_classification: "phi".to_string(),
        };
        let hash = payload_hash(&metadata, &payload).unwrap();
        let sig_bytes = sign_with(&sk, &hash);
        let claim = SignedClaim {
            metadata,
            payload,
            signature: SignatureEnvelope {
                algorithm: "ml-dsa-87".to_string(),
                public_key_id: "tenant-key".to_string(),
                bytes_hex: hex::encode(sig_bytes),
            },
        };
        let claim_payload = claim.verified_payload(&verifier, 1_500).unwrap();
        assert_eq!(claim_payload.claim_id, "claim-001");
        assert!(claim_payload.subject_hash.starts_with("hmac:"));
    }

    #[test]
    fn accept_all_verifier_passes_any_signature() {
        let bundle = SignedPolicyBundle {
            metadata: sample_metadata(1_000, 2_000),
            payload: sample_payload(),
            signature: SignatureEnvelope {
                algorithm: "ml-dsa-87".to_string(),
                public_key_id: "anything".to_string(),
                bytes_hex: hex::encode(vec![0u8; MLDSA87_SIG_LEN]),
            },
        };
        let v = AcceptAllBundleVerifier;
        assert!(bundle.verified_payload(&v, 1_500).is_ok());
    }

    #[test]
    fn reject_all_verifier_fails_any_signature() {
        let (_pk, sk) = fresh_keypair();
        let bundle =
            build_signed_bundle(sample_metadata(1_000, 2_000), sample_payload(), &sk, "tb");
        let v = RejectAllBundleVerifier;
        let err = bundle.verified_payload(&v, 1_500).unwrap_err();
        assert_eq!(err, BundleError::InvalidSignature);
    }

    #[test]
    fn anchor_count_reflects_registry() {
        let v = MlDsaBundleVerifier::new()
            .with_anchor("a", vec![0u8; MLDSA87_PK_LEN])
            .with_anchor("b", vec![0u8; MLDSA87_PK_LEN]);
        assert_eq!(v.anchor_count(), 2);
    }
}
