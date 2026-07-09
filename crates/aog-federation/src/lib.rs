//! `aog-federation` — cross-air-gap federation of Loom estates by **signed
//! removable-media snapshots**, no network. A source estate bundles the policy it
//! wants an air-gapped peer to adopt and the revocations it must honor into a
//! [`FederationSnapshot`], ML-DSA-signs it over its canonical payload, and writes
//! it to media ([`to_media`]). The peer reads the bytes ([`from_media`]), verifies
//! with the source's public key **alone** (offline, [`verify_snapshot`]), refuses
//! a stale replay (anti-rollback, [`Peer::accept`]), and applies the policy and
//! revocations. Nothing crosses a network — the media artifact is the whole
//! channel. Extends F8's signed-revocation transport to full policy federation.
//!
//! Trust posture: verification is asymmetric crypto against a pre-shared source
//! public key; a wrong key, tampered payload, or replayed older version all fail
//! closed. A snapshot never carries a secret — a revocation names a token/subject,
//! a policy names rules; the peer resolves credentials from its own OpenBao.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use fabric_contracts::Signature;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;

use aog_estate::{PolicyMode, PolicyRule, RevocationTarget};

/// A federation failure on the producing side (signing / serialization).
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error("serialize: {0}")]
    Serialize(String),
    #[error("hash: {0}")]
    Hash(String),
    #[error("sign: {0}")]
    Sign(String),
}

/// A policy carried across the air gap — the `PolicyBundle` content a peer adopts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedPolicy {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub mode: PolicyMode,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

/// A revocation carried across the air gap — a `RevocationIntent` the peer honors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedRevocation {
    pub target: RevocationTarget,
    #[serde(default)]
    pub reason: String,
}

/// The signed federation artifact written to removable media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationSnapshot {
    /// Monotonic version — a peer refuses one `<=` the last it applied.
    pub version: u64,
    /// The source estate's id (provenance).
    pub source: String,
    pub policies: Vec<FederatedPolicy>,
    pub revocations: Vec<FederatedRevocation>,
    pub signature: Signature,
}

/// BLAKE3-32 over the canonical payload with `signature` cleared — the exact bytes
/// signed and verified.
fn signing_hash(snapshot: &FederationSnapshot) -> Result<[u8; 32], FederationError> {
    let mut v =
        serde_json::to_value(snapshot).map_err(|e| FederationError::Serialize(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| FederationError::Hash(e.to_string()))
}

/// Sign a federation snapshot into its distributable, media-ready form.
///
/// # Errors
/// [`FederationError`] on canonical serialization or signer failure.
pub fn sign_snapshot(
    version: u64,
    source: impl Into<String>,
    policies: Vec<FederatedPolicy>,
    revocations: Vec<FederatedRevocation>,
    signer: &dyn Signer,
) -> Result<FederationSnapshot, FederationError> {
    let mut snapshot = FederationSnapshot {
        version,
        source: source.into(),
        policies,
        revocations,
        signature: Signature {
            alg: signer.algorithm().to_owned(),
            key_id: signer.key_id().to_owned(),
            value: String::new(),
        },
    };
    let hash = signing_hash(&snapshot)?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| FederationError::Sign(e.to_string()))?;
    snapshot.signature.value = hex::encode(sig);
    Ok(snapshot)
}

/// Verify a snapshot's signature under `public_key` alone — the check an offline
/// peer runs. Wrong key, tampered content, or a malformed signature all fail
/// closed.
///
/// # Errors
/// [`FederationReject::BadSignature`] if the signature is malformed or invalid.
pub fn verify_snapshot(
    snapshot: &FederationSnapshot,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), FederationReject> {
    let Ok(hash) = signing_hash(snapshot) else {
        return Err(FederationReject::BadSignature);
    };
    let Ok(sig) = hex::decode(&snapshot.signature.value) else {
        return Err(FederationReject::BadSignature);
    };
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(FederationReject::BadSignature),
    }
}

/// Serialize a snapshot to the bytes written on removable media.
///
/// # Errors
/// [`FederationError::Serialize`] on serialization failure.
pub fn to_media(snapshot: &FederationSnapshot) -> Result<Vec<u8>, FederationError> {
    serde_json::to_vec(snapshot).map_err(|e| FederationError::Serialize(e.to_string()))
}

/// Read a snapshot from media bytes (unverified until [`verify_snapshot`] /
/// [`Peer::accept`]).
///
/// # Errors
/// [`FederationError::Serialize`] if the bytes are not a snapshot.
pub fn from_media(bytes: &[u8]) -> Result<FederationSnapshot, FederationError> {
    serde_json::from_slice(bytes).map_err(|e| FederationError::Serialize(e.to_string()))
}

/// Why a peer refused a federation snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederationReject {
    /// The signature did not verify under the source's public key.
    BadSignature,
    /// A validly-signed but stale snapshot: its version does not exceed the one
    /// already applied. Refused so a replay cannot re-apply a superseded policy or
    /// un-revoke a token.
    Stale { applied: u64, offered: u64 },
}

impl std::fmt::Display for FederationReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => write!(f, "federation snapshot signature failed verification"),
            Self::Stale { applied, offered } => {
                write!(
                    f,
                    "stale federation snapshot v{offered} <= applied v{applied}"
                )
            }
        }
    }
}

/// What a peer applies from an accepted snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub policies: Vec<FederatedPolicy>,
    pub revocations: Vec<FederatedRevocation>,
}

/// The receiving (air-gapped) peer's federation view: the source public key it
/// trusts and the last snapshot version it applied. [`accept`](Peer::accept)
/// verifies a snapshot offline and advances the version, so a replayed older
/// artifact can never re-apply a superseded policy or un-revoke a token.
pub struct Peer {
    public_key: Vec<u8>,
    verifier: MlDsa87Verifier,
    applied_version: u64,
}

impl Peer {
    /// A peer trusting `source_public_key`, having applied nothing yet.
    #[must_use]
    pub fn new(source_public_key: Vec<u8>) -> Self {
        Self {
            public_key: source_public_key,
            verifier: MlDsa87Verifier,
            applied_version: 0,
        }
    }

    /// The last federation version this peer applied.
    #[must_use]
    pub fn applied_version(&self) -> u64 {
        self.applied_version
    }

    /// Verify `snapshot` offline and, if it advances the applied version, return
    /// the policies + revocations to apply and record the new version.
    ///
    /// # Errors
    /// [`FederationReject`] on a bad signature or a stale replay.
    pub fn accept(&mut self, snapshot: FederationSnapshot) -> Result<Applied, FederationReject> {
        verify_snapshot(&snapshot, &self.verifier, &self.public_key)?;
        if snapshot.version <= self.applied_version {
            return Err(FederationReject::Stale {
                applied: self.applied_version,
                offered: snapshot.version,
            });
        }
        self.applied_version = snapshot.version;
        Ok(Applied {
            policies: snapshot.policies,
            revocations: snapshot.revocations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use aog_estate::RevocationTarget;
    use fabric_contracts::RoutingDecision;
    use fabric_crypto::providers::RustCryptoMlDsa87;

    fn signer() -> Arc<dyn Signer> {
        Arc::new(RustCryptoMlDsa87::generate("loom-h5-source").unwrap())
    }

    fn policy() -> FederatedPolicy {
        FederatedPolicy {
            name: "phi-local-only".to_owned(),
            version: 3,
            mode: PolicyMode::Enforce,
            rules: vec![PolicyRule {
                name: "phi".to_owned(),
                effect: RoutingDecision::LocalOnly,
                when: "classification>=controlled".to_owned(),
            }],
        }
    }

    fn revocation() -> FederatedRevocation {
        FederatedRevocation {
            target: RevocationTarget::Token("tok-compromised".to_owned()),
            reason: "rotated".to_owned(),
        }
    }

    #[test]
    fn a_policy_and_a_revocation_cross_the_air_gap_and_apply() {
        let s = signer();
        let source_pubkey = s.public_key().to_vec();

        // Source estate: bundle a policy + a revocation, sign, write to media.
        let snapshot =
            sign_snapshot(7, "site-a", vec![policy()], vec![revocation()], s.as_ref()).unwrap();
        let media: Vec<u8> = to_media(&snapshot).unwrap();

        // ── Air gap: only the bytes cross. The peer verifies with the public key
        // alone (offline) and applies.
        let received = from_media(&media).unwrap();
        let mut peer = Peer::new(source_pubkey);
        let applied = peer.accept(received).expect("verified snapshot applies");
        assert_eq!(
            applied.policies,
            vec![policy()],
            "the policy crossed and applies"
        );
        assert_eq!(
            applied.revocations,
            vec![revocation()],
            "the revocation crossed and applies"
        );
        assert_eq!(peer.applied_version(), 7);
    }

    #[test]
    fn a_wrong_source_key_is_refused() {
        let snapshot =
            sign_snapshot(1, "site-a", vec![policy()], vec![], signer().as_ref()).unwrap();
        let mut peer = Peer::new(signer().public_key().to_vec()); // different key
        assert_eq!(peer.accept(snapshot), Err(FederationReject::BadSignature));
    }

    #[test]
    fn a_tampered_snapshot_is_refused() {
        let s = signer();
        let mut snapshot = sign_snapshot(1, "site-a", vec![policy()], vec![], s.as_ref()).unwrap();
        // Tamper after signing: change the policy mode.
        snapshot.policies[0].mode = PolicyMode::Shadow;
        let mut peer = Peer::new(s.public_key().to_vec());
        assert_eq!(peer.accept(snapshot), Err(FederationReject::BadSignature));
    }

    #[test]
    fn a_stale_replay_cannot_downgrade() {
        let s = signer();
        let pubkey = s.public_key().to_vec();
        let v2 =
            sign_snapshot(2, "site-a", vec![policy()], vec![revocation()], s.as_ref()).unwrap();
        let v1 = sign_snapshot(1, "site-a", vec![], vec![], s.as_ref()).unwrap();

        let mut peer = Peer::new(pubkey);
        peer.accept(v2).expect("v2 applies");
        // Replaying v1 (no revocation) must not un-revoke or downgrade.
        assert_eq!(
            peer.accept(v1),
            Err(FederationReject::Stale {
                applied: 2,
                offered: 1
            })
        );
        assert_eq!(peer.applied_version(), 2);
    }
}
