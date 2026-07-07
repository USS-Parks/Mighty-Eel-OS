//! `fabric-revocation` — signed revocation snapshots.
//!
//! A snapshot lists revoked token ids, subjects, signing keys, and bundle
//! versions, and is ML-DSA-signed (via `fabric-crypto`) so an appliance can
//! verify and apply it **offline** — even from removable media in an air-gap.
//! [`emergency`] snapshots are short-TTL, out-of-band revocations applied on the
//! next poll regardless of the normal cadence.

use fabric_contracts::{Signature, TrustToken};
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;
use serde::{Deserialize, Serialize};

/// A revocation snapshot. Signed over its canonical payload (signature excluded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationSnapshot {
    /// Stable snapshot id.
    pub snapshot_id: String,
    /// Issue time (RFC3339).
    pub issued_at: String,
    /// Expiry (RFC3339).
    pub expires_at: String,
    /// Revoked trust-token ids.
    #[serde(default)]
    pub revoked_tokens: Vec<String>,
    /// Revoked subject hashes.
    #[serde(default)]
    pub revoked_subjects: Vec<String>,
    /// Revoked signing-key ids.
    #[serde(default)]
    pub revoked_signing_keys: Vec<String>,
    /// Revoked bundle versions.
    #[serde(default)]
    pub revoked_bundle_versions: Vec<String>,
    /// Revoked tenants — every token bound to one is revoked (plan R2).
    #[serde(default)]
    pub revoked_tenants: Vec<String>,
    /// Revoked issuers (plan R2).
    #[serde(default)]
    pub revoked_issuers: Vec<String>,
    /// Revoked service identities (plan R2).
    #[serde(default)]
    pub revoked_service_identities: Vec<String>,
    /// Whether this is an out-of-band emergency snapshot.
    #[serde(default)]
    pub emergency: bool,
    /// Signature over the canonical payload.
    pub signature: Signature,
}

impl RevocationSnapshot {
    /// A new unsigned snapshot with an empty signature. Sign it with [`sign`].
    #[must_use]
    pub fn new(
        snapshot_id: impl Into<String>,
        issued_at: impl Into<String>,
        expires_at: impl Into<String>,
    ) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            issued_at: issued_at.into(),
            expires_at: expires_at.into(),
            revoked_tokens: Vec::new(),
            revoked_subjects: Vec::new(),
            revoked_signing_keys: Vec::new(),
            revoked_bundle_versions: Vec::new(),
            revoked_tenants: Vec::new(),
            revoked_issuers: Vec::new(),
            revoked_service_identities: Vec::new(),
            emergency: false,
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        }
    }

    /// Mark this snapshot as an emergency (out-of-band) revocation.
    #[must_use]
    pub fn emergency(mut self) -> Self {
        self.emergency = true;
        self
    }

    /// Is `token_id` revoked by this snapshot?
    #[must_use]
    pub fn is_token_revoked(&self, token_id: &str) -> bool {
        self.revoked_tokens.iter().any(|t| t == token_id)
    }

    /// Is `subject_hash` revoked by this snapshot?
    #[must_use]
    pub fn is_subject_revoked(&self, subject_hash: &str) -> bool {
        self.revoked_subjects.iter().any(|s| s == subject_hash)
    }

    /// Is signing key `key_id` revoked by this snapshot?
    #[must_use]
    pub fn is_key_revoked(&self, key_id: &str) -> bool {
        self.revoked_signing_keys.iter().any(|k| k == key_id)
    }

    /// Is bundle `version` revoked by this snapshot?
    #[must_use]
    pub fn is_bundle_revoked(&self, version: &str) -> bool {
        self.revoked_bundle_versions.iter().any(|v| v == version)
    }

    /// Is `tenant_id` revoked by this snapshot?
    #[must_use]
    pub fn is_tenant_revoked(&self, tenant_id: &str) -> bool {
        self.revoked_tenants.iter().any(|t| t == tenant_id)
    }

    /// Is `issuer` revoked by this snapshot?
    #[must_use]
    pub fn is_issuer_revoked(&self, issuer: &str) -> bool {
        self.revoked_issuers.iter().any(|i| i == issuer)
    }

    /// Is `service_identity` revoked by this snapshot?
    #[must_use]
    pub fn is_service_identity_revoked(&self, service_identity: &str) -> bool {
        self.revoked_service_identities
            .iter()
            .any(|s| s == service_identity)
    }

    /// The complete revocation predicate (plan R2): does this snapshot revoke
    /// `token` on **any** dimension — token id, subject hash, signing key,
    /// issuer, bundle version, tenant, or service identity? Returns the dimension
    /// that matched (for receipts), or `None`.
    #[must_use]
    pub fn revokes(&self, token: &TrustToken) -> Option<&'static str> {
        if self.is_token_revoked(&token.token_id) {
            return Some("token_id");
        }
        if self.is_subject_revoked(&token.subject_hash) {
            return Some("subject_hash");
        }
        if self.is_key_revoked(&token.signature.key_id) {
            return Some("signing_key");
        }
        if self.is_issuer_revoked(&token.issuer) {
            return Some("issuer");
        }
        if self.is_bundle_revoked(&token.trust_bundle_version) {
            return Some("bundle_version");
        }
        if self.is_tenant_revoked(&token.tenant_id) {
            return Some("tenant");
        }
        if let Some(svc) = &token.service_identity
            && self.is_service_identity_revoked(svc)
        {
            return Some("service_identity");
        }
        None
    }
}

/// Failures from revocation operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RevocationError {
    /// Canonical serialization failed.
    #[error("canonical serialization failed: {0}")]
    Serialize(String),
    /// The signer failed.
    #[error("signing failed: {0}")]
    Sign(String),
    /// The signature string was not valid hex.
    #[error("signature is not valid hex")]
    MalformedSignature,
    /// The signature did not verify.
    #[error("signature failed verification")]
    InvalidSignature,
}

/// BLAKE3-32 over the canonical payload (signature field removed).
fn signing_hash(snapshot: &RevocationSnapshot) -> Result<[u8; 32], RevocationError> {
    let mut v =
        serde_json::to_value(snapshot).map_err(|e| RevocationError::Serialize(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| RevocationError::Serialize(e.to_string()))
}

/// Sign `snapshot` over its canonical payload.
///
/// # Errors
/// Returns [`RevocationError`] if serialization or signing fails.
pub fn sign(
    mut snapshot: RevocationSnapshot,
    signer: &dyn Signer,
) -> Result<RevocationSnapshot, RevocationError> {
    snapshot.signature = Signature {
        alg: signer.algorithm().to_string(),
        key_id: signer.key_id().to_string(),
        value: String::new(),
    };
    let hash = signing_hash(&snapshot)?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| RevocationError::Sign(e.to_string()))?;
    snapshot.signature.value = hex::encode(sig);
    Ok(snapshot)
}

/// Verify a snapshot's signature.
///
/// # Errors
/// Returns [`RevocationError::MalformedSignature`] or [`RevocationError::InvalidSignature`].
pub fn verify(
    snapshot: &RevocationSnapshot,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), RevocationError> {
    let hash = signing_hash(snapshot)?;
    let sig =
        hex::decode(&snapshot.signature.value).map_err(|_| RevocationError::MalformedSignature)?;
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(RevocationError::InvalidSignature),
    }
}
