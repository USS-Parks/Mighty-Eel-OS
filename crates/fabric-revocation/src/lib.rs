//! `fabric-revocation` — signed revocation snapshots.
//!
//! A snapshot lists revoked token ids, subjects, signing keys, and bundle
//! versions, and is ML-DSA-signed (via `fabric-crypto`) so an appliance can
//! verify and apply it **offline** — even from removable media in an air-gap.
//! [`emergency`] snapshots are short-TTL, out-of-band revocations applied on the
//! next poll regardless of the normal cadence.

use chrono::{DateTime, Utc};
use fabric_contracts::Signature;
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
    /// A snapshot timestamp was not valid RFC3339.
    #[error("timestamp is not valid RFC3339: {0}")]
    BadTimestamp(String),
    /// The snapshot is expired at the checked time.
    #[error("revocation snapshot is expired")]
    Expired,
    /// The snapshot would roll the store back to an older (or equal, non-emergency)
    /// snapshot — refused; the last-known-good snapshot is retained.
    #[error("revocation snapshot rollback refused")]
    Rollback,
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

fn parse_ts(s: &str) -> Result<DateTime<Utc>, RevocationError> {
    Ok(DateTime::parse_from_rfc3339(s)
        .map_err(|_| RevocationError::BadTimestamp(s.to_string()))?
        .with_timezone(&Utc))
}

/// A verified, monotonic revocation snapshot store (R1). Replacement is
/// **anti-rollback**: a new snapshot must verify under the trust anchor, be
/// unexpired, and be strictly newer than the current one (by `issued_at`) — an
/// emergency snapshot may replace at an equal timestamp. On any failure the
/// current snapshot is retained (last-known-good), so a stale, expired, or forged
/// update can never blind the store.
#[derive(Debug, Default)]
pub struct RevocationStore {
    current: Option<RevocationSnapshot>,
}

impl RevocationStore {
    /// An empty store (no revocations in force yet).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current in-force snapshot, if one is installed.
    #[must_use]
    pub fn current(&self) -> Option<&RevocationSnapshot> {
        self.current.as_ref()
    }

    /// Install `snapshot` if it verifies under `public_key`, is unexpired at `now`,
    /// and is newer than the current one. Refuses a rollback and keeps the
    /// last-known-good snapshot on any failure.
    ///
    /// # Errors
    /// [`RevocationError::InvalidSignature`], [`Expired`](RevocationError::Expired),
    /// [`Rollback`](RevocationError::Rollback), or
    /// [`BadTimestamp`](RevocationError::BadTimestamp).
    pub fn update(
        &mut self,
        snapshot: RevocationSnapshot,
        verifier: &dyn Verifier,
        public_key: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), RevocationError> {
        verify(&snapshot, verifier, public_key)?;
        if parse_ts(&snapshot.expires_at)? <= now {
            return Err(RevocationError::Expired);
        }
        if let Some(cur) = &self.current {
            let cur_issued = parse_ts(&cur.issued_at)?;
            let new_issued = parse_ts(&snapshot.issued_at)?;
            let newer = new_issued > cur_issued || (new_issued == cur_issued && snapshot.emergency);
            if !newer {
                return Err(RevocationError::Rollback);
            }
        }
        self.current = Some(snapshot);
        Ok(())
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn now() -> DateTime<Utc> {
        parse_ts("2026-07-03T12:00:00Z").unwrap()
    }

    fn snap(
        id: &str,
        issued: &str,
        expires: &str,
        signer: &RustCryptoMlDsa87,
    ) -> RevocationSnapshot {
        sign(RevocationSnapshot::new(id, issued, expires), signer).unwrap()
    }

    #[test]
    fn install_verifies_and_rejects_rollback_expiry_and_forgery() {
        let anchor = RustCryptoMlDsa87::generate("anchor").unwrap();
        let pk = anchor.public_key().to_vec();
        let mut store = RevocationStore::new();

        // A valid snapshot installs, and a newer one replaces it.
        let s1 = snap(
            "s1",
            "2026-07-03T10:00:00Z",
            "2026-07-04T00:00:00Z",
            &anchor,
        );
        store.update(s1, &MlDsa87Verifier, &pk, now()).unwrap();
        let s2 = snap(
            "s2",
            "2026-07-03T11:00:00Z",
            "2026-07-04T00:00:00Z",
            &anchor,
        );
        store.update(s2, &MlDsa87Verifier, &pk, now()).unwrap();
        assert_eq!(store.current().unwrap().snapshot_id, "s2");

        // An older snapshot is a rollback → refused; s2 retained.
        let old = snap(
            "old",
            "2026-07-03T09:00:00Z",
            "2026-07-04T00:00:00Z",
            &anchor,
        );
        assert_eq!(
            store.update(old, &MlDsa87Verifier, &pk, now()),
            Err(RevocationError::Rollback)
        );
        assert_eq!(store.current().unwrap().snapshot_id, "s2");

        // A newer-but-expired snapshot is refused.
        let expired = snap(
            "exp",
            "2026-07-03T11:30:00Z",
            "2026-07-03T11:45:00Z",
            &anchor,
        );
        assert_eq!(
            store.update(expired, &MlDsa87Verifier, &pk, now()),
            Err(RevocationError::Expired)
        );

        // A forged (attacker-key) snapshot is refused; last-known-good retained.
        let attacker = RustCryptoMlDsa87::generate("evil").unwrap();
        let forged = snap(
            "forged",
            "2026-07-03T13:00:00Z",
            "2026-07-04T00:00:00Z",
            &attacker,
        );
        assert_eq!(
            store.update(forged, &MlDsa87Verifier, &pk, now()),
            Err(RevocationError::InvalidSignature)
        );
        assert_eq!(store.current().unwrap().snapshot_id, "s2");
    }
}
