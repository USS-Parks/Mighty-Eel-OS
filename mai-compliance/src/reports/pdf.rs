//! Report certification ("PDF")
//!
//! Wraps a generated [`super::engine::ReportDocument`] in a signed
//! envelope. The certification layer is intentionally output-format
//! agnostic: the canonical bytes that get signed are the report's
//! **JSON** rendering (the format that round-trips losslessly), and
//! the signature can be verified by any party that has the
//! corresponding ML-DSA-87 public key registered with their
//! [`crate::bundle::BundleVerifier`].
//!
//! The signing primitive matches the bundle verifier and the
//! audit-chain signer: signer receives a 32-byte BLAKE3 digest
//! and returns an ML-DSA-87 signature. Verification therefore uses
//! the same anchor registry as everything else in the trust stack —
//! there is exactly one signing primitive in the system.
//!
//! ## "PDF" naming
//!
//! This module is deliberately named `pdf.rs`. A real PDF binary is a
//! presentation concern, not a verification concern, so the module
//! exposes the *signed text body* + *certification metadata* and
//! lets the dashboard render it into whatever container
//! (real PDF, HTML preview, e-mail attachment) it wants. The signed
//! artefact is the [`CertifiedReport`].

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use crate::audit::CHAIN_HASH_LEN;
use crate::bundle::{BundleError, BundleVerifier};

use super::engine::{ReportDocument, ReportFormat, ReportMetadata};

/// ML-DSA-87 signature length (matches
/// [`crate::audit::SIGNATURE_LEN`]).
pub const REPORT_SIGNATURE_LEN: usize = 4627;

/// Pluggable signer for the certification page. Same shape as
/// [`crate::audit::ChainSigner`]; restated here so the reports
/// subsystem has its own trait the audit subsystem doesn't have to
/// re-export.
pub trait ReportSigner: Send + Sync + std::fmt::Debug {
    /// Identifier of the signing key (must match a registered
    /// anchor in the verifier's registry).
    fn key_id(&self) -> &str;

    /// Sign the given 32-byte BLAKE3 digest. Returning `None` is a
    /// valid bring-up state — the [`CertifiedReport`] will still
    /// embed the digest, just without an ML-DSA signature, and
    /// the verifier surfaces that as `SignatureMissing`.
    fn sign(&self, payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>>;
}

/// No-op signer for bring-up and tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullReportSigner;

impl ReportSigner for NullReportSigner {
    fn key_id(&self) -> &'static str {
        ""
    }
    fn sign(&self, _payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>> {
        None
    }
}

/// ML-DSA-87 signer for production. Holds a 4896-byte signing key.
#[derive(Debug, Clone)]
pub struct MlDsaReportSigner {
    key_id: String,
    signing_key_bytes: Vec<u8>,
}

impl MlDsaReportSigner {
    /// Construct from raw ML-DSA-87 signing key bytes.
    pub fn new(key_id: impl Into<String>, signing_key_bytes: Vec<u8>) -> Self {
        Self {
            key_id: key_id.into(),
            signing_key_bytes,
        }
    }

    /// Generate a fresh keypair (test-only). Returns
    /// `(signer, public_key_bytes)`.
    #[cfg(test)]
    pub fn generate<R: rand::RngCore + rand::CryptoRng>(
        key_id: impl Into<String>,
        rng: &mut R,
    ) -> (Self, Vec<u8>) {
        use ml_dsa::{KeyGen, MlDsa87};
        let kp = MlDsa87::key_gen(rng);
        let sk_bytes = kp.signing_key().encode().to_vec();
        let pk_bytes = kp.verifying_key().encode().to_vec();
        (Self::new(key_id, sk_bytes), pk_bytes)
    }
}

impl ReportSigner for MlDsaReportSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn sign(&self, payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>> {
        use ml_dsa::signature::Signer;
        use ml_dsa::{EncodedSigningKey, MlDsa87, Signature, SigningKey};

        const SIGNING_KEY_LEN: usize = 4896;
        if self.signing_key_bytes.len() != SIGNING_KEY_LEN {
            return None;
        }
        let sk_arr: &[u8; SIGNING_KEY_LEN] = self.signing_key_bytes.as_slice().try_into().ok()?;
        let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(payload_hash);
        Some(sig.encode().to_vec())
    }
}

/// The certification envelope a regulator receives.
///
/// Holds the original [`ReportDocument`] alongside the signature
/// material so the consumer can:
///
/// 1. Recompute the BLAKE3 of the report's JSON rendering.
/// 2. Compare against [`Self::content_hash`].
/// 3. Verify [`Self::signature`] against the registered public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedReport {
    /// The underlying document. Note: the *signed payload* is the
    /// document's **JSON** body bytes — not whatever `format` was
    /// requested. Two `CertifiedReport`s with different output
    /// formats but identical content have identical signatures.
    pub document: ReportDocument,
    /// Format the caller originally rendered the body in.
    pub rendered_format: ReportFormat,
    /// Hex-encoded BLAKE3 of the canonical JSON bytes.
    pub content_hash_hex: String,
    /// Identifier of the signing key (matches an anchor registered
    /// with the [`BundleVerifier`]).
    pub signing_key_id: String,
    /// Optional ML-DSA-87 signature, hex-encoded. `None` when the
    /// signer was a [`NullReportSigner`].
    pub signature_hex: Option<String>,
    /// Watermark text included on the certification page.
    pub watermark: String,
    /// Wall-clock nanoseconds the certification was produced.
    pub certified_at_unix_nanos: u64,
}

impl CertifiedReport {
    /// Borrow the report metadata.
    pub fn metadata(&self) -> &ReportMetadata {
        &self.document.metadata
    }

    /// True when a signature is embedded.
    pub fn is_signed(&self) -> bool {
        self.signature_hex.is_some()
    }
}

/// Produces [`CertifiedReport`]s from generated documents.
#[derive(Debug)]
pub struct ReportCertifier {
    signer: Box<dyn ReportSigner>,
    watermark: String,
}

impl ReportCertifier {
    /// Build a certifier with the given signer and default watermark.
    pub fn new(signer: Box<dyn ReportSigner>) -> Self {
        Self {
            signer,
            watermark: "Generated by Island Mountain MAI — Lamprey Compliance Layer".into(),
        }
    }

    /// Override the watermark text.
    pub fn with_watermark(mut self, watermark: impl Into<String>) -> Self {
        self.watermark = watermark.into();
        self
    }

    /// Wrap a document in a [`CertifiedReport`].
    pub fn certify(
        &self,
        document: ReportDocument,
        now_unix_nanos: u64,
    ) -> Result<CertifiedReport, CertifyError> {
        let canonical = serde_json::to_vec(&document.payload).map_err(CertifyError::Serialize)?;
        let mut h = Hasher::new();
        h.update(&canonical);
        let digest = *h.finalize().as_bytes();
        let signature = self.signer.sign(&digest);
        Ok(CertifiedReport {
            rendered_format: document.format,
            document,
            content_hash_hex: hex::encode(digest),
            signing_key_id: self.signer.key_id().to_string(),
            signature_hex: signature.map(hex::encode),
            watermark: self.watermark.clone(),
            certified_at_unix_nanos: now_unix_nanos,
        })
    }
}

/// Verifies a [`CertifiedReport`] against the supplied verifier.
///
/// `Ok(())` means the content hash matched and (if a signature was
/// present) the verifier accepted it. `Err` describes the failure.
pub fn verify_certified_report<V: BundleVerifier>(
    report: &CertifiedReport,
    verifier: Option<&V>,
) -> Result<(), VerifyError> {
    let canonical = serde_json::to_vec(&report.document.payload).map_err(VerifyError::Serialize)?;
    let mut h = Hasher::new();
    h.update(&canonical);
    let digest = *h.finalize().as_bytes();
    let expected = hex::encode(digest);
    if expected != report.content_hash_hex {
        return Err(VerifyError::ContentHashMismatch {
            expected: expected.clone(),
            actual: report.content_hash_hex.clone(),
        });
    }
    match (&report.signature_hex, verifier) {
        (Some(sig_hex), Some(v)) => {
            let sig = hex::decode(sig_hex).map_err(|_| VerifyError::InvalidSignatureEncoding)?;
            v.verify(&digest, &sig, &report.signing_key_id)
                .map_err(VerifyError::Verifier)
        }
        (Some(_), None) => Err(VerifyError::VerifierMissing),
        (None, _) => Err(VerifyError::SignatureMissing),
    }
}

/// Errors raised by [`ReportCertifier::certify`].
#[derive(Debug, thiserror::Error)]
pub enum CertifyError {
    /// Could not serialise the payload to canonical JSON.
    #[error("payload serialisation failed: {0}")]
    Serialize(#[source] serde_json::Error),
}

/// Errors raised by [`verify_certified_report`].
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Could not serialise the payload to canonical JSON.
    #[error("payload serialisation failed: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Recomputed content hash does not match the embedded one.
    #[error("content hash mismatch: expected {expected}, got {actual}")]
    ContentHashMismatch {
        /// Hash computed from the payload.
        expected: String,
        /// Hash embedded in the report.
        actual: String,
    },
    /// Signature bytes were not valid hex.
    #[error("signature is not valid hex")]
    InvalidSignatureEncoding,
    /// Caller passed `None` for the verifier but the report carries
    /// a signature that needs checking.
    #[error("verifier required: report carries a signature")]
    VerifierMissing,
    /// Report does not carry a signature; verification is only
    /// content-hash based when the signature is absent.
    #[error("no signature present on certified report")]
    SignatureMissing,
    /// Underlying ML-DSA verifier rejected the signature.
    #[error("signature verification failed: {0}")]
    Verifier(#[source] BundleError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditLog;
    use crate::bundle::MlDsaBundleVerifier;
    use crate::reports::api::ReportRequest;
    use crate::reports::engine::ReportEngine;
    use crate::reports::templates::SystemActivitySummary;

    fn make_document() -> ReportDocument {
        let audit = AuditLog::default();
        let engine = ReportEngine::new(audit);
        let request = ReportRequest {
            report_type: super::super::engine::ReportType::SystemActivity,
            from_unix_nanos: 0,
            to_unix_nanos: u64::MAX,
            tenant: None,
        };
        engine
            .generate(
                &SystemActivitySummary,
                &request,
                ReportFormat::Json,
                "test-policy",
                42,
            )
            .expect("generate")
    }

    #[test]
    fn null_signer_produces_unsigned_certified_report() {
        let doc = make_document();
        let certifier = ReportCertifier::new(Box::new(NullReportSigner));
        let certified = certifier.certify(doc, 100).expect("certify");
        assert!(!certified.is_signed());
        assert_eq!(certified.content_hash_hex.len(), 64);
        assert!(certified.watermark.contains("Island Mountain"));
        let res = verify_certified_report::<MlDsaBundleVerifier>(&certified, None);
        assert!(matches!(res, Err(VerifyError::SignatureMissing)));
    }

    #[test]
    fn ml_dsa_signer_round_trip_verifies() {
        let mut rng = rand::thread_rng();
        let (signer, pk_bytes) = MlDsaReportSigner::generate("rpt-key-1", &mut rng);
        let verifier = MlDsaBundleVerifier::new().with_anchor("rpt-key-1", pk_bytes);

        let doc = make_document();
        let certifier = ReportCertifier::new(Box::new(signer));
        let certified = certifier.certify(doc, 100).expect("certify");
        assert!(certified.is_signed());
        verify_certified_report(&certified, Some(&verifier)).expect("verify");
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let mut rng = rand::thread_rng();
        let (signer, pk_bytes) = MlDsaReportSigner::generate("rpt-key-2", &mut rng);
        let verifier = MlDsaBundleVerifier::new().with_anchor("rpt-key-2", pk_bytes);

        let doc = make_document();
        let certifier = ReportCertifier::new(Box::new(signer));
        let mut certified = certifier.certify(doc, 100).expect("certify");
        certified.document.payload.summary = "tampered".into();
        let res = verify_certified_report(&certified, Some(&verifier));
        assert!(matches!(res, Err(VerifyError::ContentHashMismatch { .. })));
    }
}
