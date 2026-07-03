//! `fabric-identity` — WSF workload / session / task identity.
//!
//! The signed assertion of *who or what* is acting, before any authority (a
//! trust token) is granted. This crate mints and verifies
//! `fabric_contracts::Identity` assertions (signed via `fabric-crypto`), derives
//! short-lived child identities for the loop → session → task chain, and
//! pseudonymizes subjects (via `fabric-proof`).
//!
//! PKI-leaf binding (`pki_cert_fingerprint`) is populated by the Phase-W
//! OpenBao PKI wiring; here it is carried through unchanged.

use fabric_contracts::{Identity, IdentityKind, Signature};
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;

/// Failures from the identity operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    /// The identity could not be serialized for hashing.
    #[error("canonical serialization failed: {0}")]
    Serialize(String),
    /// The signer failed.
    #[error("signing failed: {0}")]
    Sign(String),
    /// The signature string was not valid hex.
    #[error("signature is not valid hex")]
    MalformedSignature,
    /// The signature did not verify against the public key.
    #[error("signature failed verification")]
    InvalidSignature,
    /// A child identity must be `Session` or `Task` kind.
    #[error("child identity must be Session or Task kind, got {0:?}")]
    InvalidChildKind(IdentityKind),
    /// The per-tenant HMAC key was too short for pseudonymization.
    #[error("tenant HMAC key too short: {got} bytes (minimum {min})")]
    TenantKeyTooShort {
        /// Supplied key length.
        got: usize,
        /// Minimum required length.
        min: usize,
    },
}

/// BLAKE3-32 over the canonical identity payload (signature field removed).
fn signing_hash(identity: &Identity) -> Result<[u8; 32], IdentityError> {
    let mut v =
        serde_json::to_value(identity).map_err(|e| IdentityError::Serialize(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| IdentityError::Serialize(e.to_string()))
}

/// Sign `identity` over its canonical payload, returning the signed identity.
///
/// # Errors
/// Returns [`IdentityError`] if serialization or signing fails.
pub fn mint(mut identity: Identity, signer: &dyn Signer) -> Result<Identity, IdentityError> {
    identity.signature = Signature {
        alg: signer.algorithm().to_string(),
        key_id: signer.key_id().to_string(),
        value: String::new(),
    };
    let hash = signing_hash(&identity)?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| IdentityError::Sign(e.to_string()))?;
    identity.signature.value = hex::encode(sig);
    Ok(identity)
}

/// Verify an identity's signature.
///
/// # Errors
/// Returns [`IdentityError::MalformedSignature`] or [`IdentityError::InvalidSignature`].
pub fn verify(
    identity: &Identity,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), IdentityError> {
    let hash = signing_hash(identity)?;
    let sig =
        hex::decode(&identity.signature.value).map_err(|_| IdentityError::MalformedSignature)?;
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(IdentityError::InvalidSignature),
    }
}

/// Fields that vary between a parent and its derived child identity.
pub struct ChildSpec {
    /// The child's own id.
    pub identity_id: String,
    /// `Session` or `Task`.
    pub kind: IdentityKind,
    /// The child's SPIFFE id.
    pub spiffe_id: String,
    /// Issue time (RFC3339).
    pub issued_at: String,
    /// Expiry (RFC3339) — must be short.
    pub expires_at: String,
}

/// Derive a short-lived child identity (a session or task within an agent loop)
/// bound to `parent` via `parent_id`, inheriting tenant/subject/service, then
/// sign it. The child's PKI fingerprint starts empty (bound in Phase W).
///
/// # Errors
/// Returns [`IdentityError::InvalidChildKind`] if `spec.kind` is not `Session`
/// or `Task`, or a signing error.
pub fn derive_child(
    parent: &Identity,
    spec: ChildSpec,
    signer: &dyn Signer,
) -> Result<Identity, IdentityError> {
    if !matches!(spec.kind, IdentityKind::Session | IdentityKind::Task) {
        return Err(IdentityError::InvalidChildKind(spec.kind));
    }
    let child = Identity {
        identity_id: spec.identity_id,
        kind: spec.kind,
        tenant_id: parent.tenant_id.clone(),
        subject_id: parent.subject_id.clone(),
        subject_hash: parent.subject_hash.clone(),
        service_identity: parent.service_identity.clone(),
        spiffe_id: spec.spiffe_id,
        pki_cert_fingerprint: String::new(),
        parent_id: Some(parent.identity_id.clone()),
        issued_at: spec.issued_at,
        expires_at: spec.expires_at,
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    mint(child, signer)
}

/// Pseudonymize a raw subject id under a per-tenant HMAC key, producing an
/// `"hmac:"`-prefixed identifier safe for audit correlation.
///
/// # Errors
/// Returns [`IdentityError::TenantKeyTooShort`] if `tenant_key` is too short.
pub fn pseudonymize(tenant_key: &[u8], subject_id: &str) -> Result<String, IdentityError> {
    fabric_proof::hmac_subject(tenant_key, subject_id).map_err(|e| match e {
        fabric_proof::ProofError::TenantKeyTooShort { got, min } => {
            IdentityError::TenantKeyTooShort { got, min }
        }
        other => IdentityError::Serialize(other.to_string()),
    })
}
