//! `fabric-revocation` — signed revocation snapshots.
//!
//! A snapshot lists revoked token ids, subjects, signing keys, and bundle
//! versions, and is ML-DSA-signed (via `fabric-crypto`) so an appliance can
//! verify and apply it **offline** — even from removable media in an air-gap.
//! [`emergency`] snapshots are short-TTL, out-of-band revocations applied on the
//! next poll regardless of the normal cadence.

use chrono::{DateTime, Utc};
use fabric_contracts::{Signature, TrustToken, WsfPrincipal};
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;
use serde::{Deserialize, Serialize};

/// A revocation snapshot. Signed over its canonical payload (signature excluded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationSnapshot {
    /// Stable snapshot id.
    pub snapshot_id: String,
    /// Monotonic publication counter. Consumers accept a snapshot
    /// only if its sequence is strictly higher than the one they hold, so a
    /// replayed older snapshot — a stale "nothing revoked" view — is rejected.
    /// Emergency snapshots share the same counter (the publisher bumps it).
    /// Serialized only when non-zero so pre-sequence signatures keep verifying.
    #[serde(default, skip_serializing_if = "sequence_is_zero")]
    pub sequence: u64,
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
    /// Revoked tenants — every token bound to one is revoked.
    #[serde(default)]
    pub revoked_tenants: Vec<String>,
    /// Revoked issuers.
    #[serde(default)]
    pub revoked_issuers: Vec<String>,
    /// Revoked service identities.
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
            sequence: 0,
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

    /// Set the monotonic publication sequence.
    #[must_use]
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
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

    /// The complete revocation predicate: does this snapshot revoke
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

    /// Complete predicate available for a transport-authenticated principal
    /// before a trust token exists.
    #[must_use]
    pub fn revokes_principal(&self, principal: &WsfPrincipal) -> Option<&'static str> {
        if self.is_tenant_revoked(&principal.tenant_id) {
            return Some("tenant");
        }
        if self.is_subject_revoked(&principal.subject_hash) {
            return Some("subject_hash");
        }
        if let Some(service) = &principal.service_identity
            && self.is_service_identity_revoked(service)
        {
            return Some("service_identity");
        }
        None
    }
}

fn sequence_is_zero(sequence: &u64) -> bool {
    *sequence == 0
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
    /// The candidate snapshot does not advance the held sequence —
    /// a rollback / replay of older revocation state was refused.
    #[error("snapshot rollback rejected: candidate sequence {candidate} <= held {current}")]
    Rollback {
        /// Sequence of the snapshot the store holds.
        current: u64,
        /// Sequence of the rejected candidate.
        candidate: u64,
    },
}

/// Fail-closed result from consulting the current revocation state at a
/// caller-supplied trusted time.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CurrentRevocationError {
    /// No anchor-verified snapshot has been accepted yet.
    #[error("revocation state unavailable")]
    Unavailable,
    /// Sequence zero is a legacy transport value, not current production state.
    #[error("revocation snapshot has no monotonic sequence")]
    Unsequenced,
    /// The signed issue time is malformed.
    #[error("revocation snapshot issued_at is invalid")]
    InvalidIssuedAt,
    /// The signed issue time is ahead of trusted time.
    #[error("revocation snapshot is not yet valid")]
    NotYetValid,
    /// The signed expiry is malformed.
    #[error("revocation snapshot expires_at is invalid")]
    InvalidExpiresAt,
    /// Trusted time is at or beyond the signed expiry.
    #[error("revocation snapshot expired")]
    Expired,
    /// The complete predicate matched a token dimension.
    #[error("token revoked ({0})")]
    Revoked(&'static str),
}

/// Monotonic, anti-rollback revocation state for a service consumer.
///
/// [`advance`](Self::advance) accepts a candidate snapshot only if it
/// (a) verifies against the trust anchor and (b) carries a strictly higher
/// [`sequence`](RevocationSnapshot::sequence) than the held snapshot. A
/// replayed older snapshot — a stale "nothing revoked" view served by a
/// compromised or lagging distribution channel — is rejected and the held
/// state stands. Emergency snapshots participate in the same sequence space,
/// so an out-of-band revocation can never be rolled back by a slower regular
/// publication either.
///
/// The store holds only anchor-verified snapshots; consumers that are
/// configured with a store must fail closed when it is empty or its snapshot
/// has expired (freshness is the consumer's clock-aware check).
#[derive(Debug, Default)]
pub struct MonotonicRevocationStore {
    current: Option<RevocationSnapshot>,
}

impl MonotonicRevocationStore {
    /// An empty store — consumers fail closed until the first `advance`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Verify `candidate` against the trust anchor and adopt it if it strictly
    /// advances the held sequence. Returns the adopted sequence.
    ///
    /// # Errors
    /// [`RevocationError::InvalidSignature`] / [`RevocationError::MalformedSignature`]
    /// if the candidate does not verify; [`RevocationError::Rollback`] if it
    /// does not advance the held sequence (the held snapshot is kept).
    pub fn advance(
        &mut self,
        candidate: RevocationSnapshot,
        verifier: &dyn Verifier,
        public_key: &[u8],
    ) -> Result<u64, RevocationError> {
        verify(&candidate, verifier, public_key)?;
        if let Some(held) = &self.current
            && candidate.sequence <= held.sequence
        {
            return Err(RevocationError::Rollback {
                current: held.sequence,
                candidate: candidate.sequence,
            });
        }
        let sequence = candidate.sequence;
        self.current = Some(candidate);
        Ok(sequence)
    }

    /// The held snapshot, if any.
    #[must_use]
    pub fn current(&self) -> Option<&RevocationSnapshot> {
        self.current.as_ref()
    }

    /// Does the held snapshot revoke `token`? Returns the matched dimension.
    /// `None` also when the store is empty — a consumer configured with a
    /// store must treat "no snapshot" as deny via [`current`](Self::current),
    /// not via this predicate.
    #[must_use]
    pub fn revokes(&self, token: &TrustToken) -> Option<&'static str> {
        self.current.as_ref().and_then(|s| s.revokes(token))
    }

    /// Consult the anchor-verified, monotonic snapshot at `trusted_now` and
    /// apply the complete token predicate. Missing, unsequenced, malformed,
    /// future, expired, or matching state denies.
    ///
    /// # Errors
    /// A [`CurrentRevocationError`] describing the fail-closed condition.
    pub fn authorize(
        &self,
        token: &TrustToken,
        trusted_now: DateTime<Utc>,
    ) -> Result<(), CurrentRevocationError> {
        let snapshot = self.current_at(trusted_now)?;
        if let Some(dimension) = snapshot.revokes(token) {
            return Err(CurrentRevocationError::Revoked(dimension));
        }
        Ok(())
    }

    /// Apply current revocation dimensions available on a verified principal.
    pub fn authorize_principal(
        &self,
        principal: &WsfPrincipal,
        trusted_now: DateTime<Utc>,
    ) -> Result<(), CurrentRevocationError> {
        let snapshot = self.current_at(trusted_now)?;
        if let Some(dimension) = snapshot.revokes_principal(principal) {
            return Err(CurrentRevocationError::Revoked(dimension));
        }
        Ok(())
    }

    fn current_at(
        &self,
        trusted_now: DateTime<Utc>,
    ) -> Result<&RevocationSnapshot, CurrentRevocationError> {
        let snapshot = self
            .current
            .as_ref()
            .ok_or(CurrentRevocationError::Unavailable)?;
        if snapshot.sequence == 0 {
            return Err(CurrentRevocationError::Unsequenced);
        }
        let issued_at = DateTime::parse_from_rfc3339(&snapshot.issued_at)
            .map_err(|_| CurrentRevocationError::InvalidIssuedAt)?
            .with_timezone(&Utc);
        if issued_at > trusted_now {
            return Err(CurrentRevocationError::NotYetValid);
        }
        let expires_at = DateTime::parse_from_rfc3339(&snapshot.expires_at)
            .map_err(|_| CurrentRevocationError::InvalidExpiresAt)?
            .with_timezone(&Utc);
        if trusted_now >= expires_at {
            return Err(CurrentRevocationError::Expired);
        }
        Ok(snapshot)
    }
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
