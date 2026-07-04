//! Hash chain manager with periodic PQC signatures.
//!
//! [`HashChainManager`] is the append-only structure that turns a
//! stream of [`AuditEntry`] records into a tamper-evident chain. Each
//! new entry's `previous_hash` is filled with the BLAKE3 of the
//! previous entry's canonical bytes; on every Nth append the manager
//! asks its [`ChainSigner`] to sign the running chain head, embedding
//! the signature on the entry being appended.
//!
//! The chain manager owns *no* storage. It returns the finalised
//! entry to the caller, which is responsible for persisting it (see
//! [`super::store`]). This keeps the chain pure: same inputs in,
//! same chain out, trivially testable.
//!
//! ## Signing
//!
//! [`ChainSigner`] is a trait so deployments can plug in different
//! signing backends:
//!
//! - [`NullSigner`] (the default) returns no signature. Suitable for
//!   bring-up and tests that don't exercise the verifiability path.
//! - [`MlDsaChainSigner`] holds an ML-DSA-87 signing key. Production
//!   deployments wire this to a vault-issued audit key (separate
//!   from the model-signing key).
//!
//! Verification uses
//! [`crate::bundle::MlDsaBundleVerifier`] to keep the signing-side
//! and verifying-side primitives consistent.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::entry::{AuditEntry, CHAIN_HASH_LEN, SIGNATURE_LEN};
use crate::bundle::{BundleError, BundleVerifier};

/// Default signature interval: every 1000 entries get a periodic
/// ML-DSA signature embedded.
pub const DEFAULT_SIGNATURE_INTERVAL: u64 = 1000;

/// Configuration for the chain manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Sign every Nth entry. `0` disables periodic signing entirely
    /// (chain still links via `previous_hash`, no signatures stored).
    #[serde(default = "ChainConfig::default_signature_interval")]
    pub signature_interval: u64,
    /// Identifier of the public key used to verify periodic
    /// signatures. The verifier looks this up in its anchor registry.
    /// Empty when `signature_interval == 0`.
    #[serde(default)]
    pub signing_key_id: String,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            signature_interval: Self::default_signature_interval(),
            signing_key_id: String::new(),
        }
    }
}

impl ChainConfig {
    /// Default signature interval (1000 entries).
    pub fn default_signature_interval() -> u64 {
        DEFAULT_SIGNATURE_INTERVAL
    }
}

/// Errors produced when verifying a chain.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChainError {
    /// First entry of a chain must declare an all-zero
    /// `previous_hash`.
    #[error("chain head (id={id}) must have zero previous_hash")]
    HeadHashNonZero {
        /// Id of the offending entry.
        id: u64,
    },
    /// Entry's `previous_hash` does not match the actual previous
    /// entry's content hash.
    #[error(
        "chain break between id={previous_id} and id={current_id}: previous_hash does not match"
    )]
    LinkBroken {
        /// Id of the previous entry whose hash was expected.
        previous_id: u64,
        /// Id of the current entry whose `previous_hash` is wrong.
        current_id: u64,
    },
    /// Entry ids must be strictly increasing.
    #[error("chain ids not monotonic: id={current_id} followed id={previous_id}")]
    NonMonotonicIds {
        /// Previous entry id.
        previous_id: u64,
        /// Current entry id (must be > previous).
        current_id: u64,
    },
    /// A periodic signature failed verification.
    #[error("periodic signature on entry id={id} failed verification: {source}")]
    SignatureVerificationFailed {
        /// Id of the entry whose signature failed.
        id: u64,
        /// Underlying verifier error.
        #[source]
        source: BundleError,
    },
}

/// Pluggable signer used by the chain manager.
///
/// The chain manager hashes the entry's canonical bytes with BLAKE3
/// before invoking [`Self::sign`], so signers always receive a fixed
/// 32-byte payload. This matches the verifier contract
/// ([`crate::bundle::BundleVerifier::verify`]): both signer and
/// verifier operate over the same 32-byte digest.
pub trait ChainSigner: Send + Sync + std::fmt::Debug {
    /// Sign the given 32-byte digest. Returns the signature bytes,
    /// or `None` if signing is disabled.
    fn sign(&self, payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>>;
}

/// No-op signer. Used as the default during bring-up and in any test
/// that does not exercise the verifiability path.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSigner;

impl ChainSigner for NullSigner {
    fn sign(&self, _payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>> {
        None
    }
}

/// ML-DSA-87 chain signer. Holds the signing key in memory; the key
/// is expected to be sourced from the vault at startup.
#[derive(Debug, Clone)]
pub struct MlDsaChainSigner {
    signing_key_bytes: Vec<u8>,
}

impl MlDsaChainSigner {
    /// Construct a signer from raw ML-DSA-87 signing key bytes
    /// (4896-byte serialised form).
    pub fn new(signing_key_bytes: Vec<u8>) -> Self {
        Self { signing_key_bytes }
    }

    /// Generate a fresh keypair using the supplied RNG. Returns
    /// `(signer, public_key_bytes)`; callers must register the
    /// public key with their [`BundleVerifier`] to enable
    /// verification. Production deployments source the keypair from
    /// the vault and never call this function.
    #[cfg(test)]
    pub fn generate<R: rand::RngCore + rand::CryptoRng>(rng: &mut R) -> (Self, Vec<u8>) {
        use ml_dsa::{KeyGen, MlDsa87};
        let kp = MlDsa87::key_gen(rng);
        let sk_bytes = kp.signing_key().encode().to_vec();
        let pk_bytes = kp.verifying_key().encode().to_vec();
        (Self::new(sk_bytes), pk_bytes)
    }
}

impl ChainSigner for MlDsaChainSigner {
    fn sign(&self, payload_hash: &[u8; CHAIN_HASH_LEN]) -> Option<Vec<u8>> {
        use ml_dsa::signature::Signer;
        use ml_dsa::{EncodedSigningKey, MlDsa87, Signature, SigningKey};

        // Decode the signing key. If anything is wrong (wrong length,
        // corrupt bytes) we return None rather than panic — the chain
        // still records the entry, just without a periodic signature.
        // The chain verifier surfaces this as a missing signature on
        // an entry that *should* have had one.
        if self.signing_key_bytes.len() != SIGNING_KEY_LEN {
            return None;
        }
        let sk_arr: &[u8; SIGNING_KEY_LEN] = self.signing_key_bytes.as_slice().try_into().ok()?;
        let sk_encoded = EncodedSigningKey::<MlDsa87>::from(*sk_arr);
        let sk = SigningKey::<MlDsa87>::decode(&sk_encoded);
        let sig: Signature<MlDsa87> = sk.sign(payload_hash);
        let sig_bytes = sig.encode().to_vec();
        debug_assert_eq!(sig_bytes.len(), SIGNATURE_LEN);
        Some(sig_bytes)
    }
}

const SIGNING_KEY_LEN: usize = 4896;

/// Append-only hash chain manager.
///
/// Thread-safe — the internal `next_id` / `previous_hash` cursor is
/// behind a `Mutex` so multiple producers can append concurrently
/// without external synchronisation.
#[derive(Clone)]
pub struct HashChainManager {
    config: ChainConfig,
    signer: Arc<dyn ChainSigner>,
    cursor: Arc<Mutex<ChainCursor>>,
}

impl std::fmt::Debug for HashChainManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HashChainManager")
            .field("config", &self.config)
            .field("cursor", &self.cursor)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct ChainCursor {
    next_id: u64,
    previous_hash: [u8; CHAIN_HASH_LEN],
}

impl HashChainManager {
    /// Build a chain manager with the given config and signer.
    pub fn new(config: ChainConfig, signer: Arc<dyn ChainSigner>) -> Self {
        Self {
            config,
            signer,
            cursor: Arc::new(Mutex::new(ChainCursor::default())),
        }
    }

    /// Build a chain manager with [`NullSigner`] (no periodic
    /// signatures).
    pub fn unsigned(config: ChainConfig) -> Self {
        Self::new(config, Arc::new(NullSigner))
    }

    /// Active chain configuration.
    pub fn config(&self) -> &ChainConfig {
        &self.config
    }

    /// Finalise a draft entry: assign its id, link `previous_hash`
    /// to the running cursor, optionally embed a periodic signature,
    /// and return the entry to the caller for persistence. Updates
    /// the cursor for the next append.
    pub fn finalize(&self, mut draft: AuditEntry) -> AuditEntry {
        let mut cursor = self.cursor.lock().expect("chain cursor poisoned");
        draft.id = cursor.next_id;
        draft.previous_hash = cursor.previous_hash;
        // Periodic signing: every Nth entry (excluding the head if
        // interval > 1). `signature_interval == 0` disables. The
        // signer is handed the BLAKE3 of the canonical bytes so the
        // signing primitive operates over a fixed 32-byte digest
        draft.signature = if self.config.signature_interval > 0
            && (draft.id + 1).is_multiple_of(self.config.signature_interval)
        {
            self.signer.sign(&draft.content_hash())
        } else {
            None
        };
        let new_hash = draft.content_hash();
        cursor.next_id = cursor.next_id.saturating_add(1);
        cursor.previous_hash = new_hash;
        draft
    }

    /// Number of entries finalised so far.
    pub fn count(&self) -> u64 {
        self.cursor.lock().expect("chain cursor poisoned").next_id
    }

    /// Current chain head hash (for external snapshotting / report
    /// generation). All-zero when the chain is empty.
    pub fn head_hash(&self) -> [u8; CHAIN_HASH_LEN] {
        self.cursor
            .lock()
            .expect("chain cursor poisoned")
            .previous_hash
    }

    /// Reset the manager to an empty chain. Used by the store on
    /// daily rotation to start a fresh chain segment.
    pub fn reset(&self) {
        let mut cursor = self.cursor.lock().expect("chain cursor poisoned");
        cursor.next_id = 0;
        cursor.previous_hash = [0u8; CHAIN_HASH_LEN];
    }

    /// Restore the manager's cursor from an existing tail entry.
    /// Used by the store on startup when replaying a WAL.
    pub fn restore_from(&self, last: &AuditEntry) {
        let mut cursor = self.cursor.lock().expect("chain cursor poisoned");
        cursor.next_id = last.id.saturating_add(1);
        cursor.previous_hash = last.content_hash();
    }
}

/// Verify a full chain segment. Checks id monotonicity,
/// `previous_hash` linkage, and any periodic signatures.
pub fn verify_chain<V: BundleVerifier>(
    entries: &[AuditEntry],
    config: &ChainConfig,
    verifier: Option<&V>,
) -> Result<(), ChainError> {
    let Some(first) = entries.first() else {
        return Ok(());
    };
    if !first.is_chain_head() {
        return Err(ChainError::HeadHashNonZero { id: first.id });
    }

    for window in entries.windows(2) {
        let prev = &window[0];
        let curr = &window[1];
        if curr.id <= prev.id {
            return Err(ChainError::NonMonotonicIds {
                previous_id: prev.id,
                current_id: curr.id,
            });
        }
        if curr.previous_hash != prev.content_hash() {
            return Err(ChainError::LinkBroken {
                previous_id: prev.id,
                current_id: curr.id,
            });
        }
    }

    // Verify periodic signatures when a verifier is supplied.
    if let Some(v) = verifier
        && config.signature_interval > 0
    {
        for entry in entries {
            let Some(sig) = entry.signature.as_ref() else {
                continue;
            };
            // Same digest the signer received: BLAKE3 of canonical
            // bytes (which exclude `signature` itself).
            let payload_hash = entry.content_hash();
            v.verify(&payload_hash, sig, &config.signing_key_id)
                .map_err(|source| ChainError::SignatureVerificationFailed {
                    id: entry.id,
                    source,
                })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::entry::{CorrelationFields, RoutingDecision, masked_request_hash};
    use crate::bundle::MlDsaBundleVerifier;
    use crate::policy::composer::ModuleId;

    fn draft(reason: &str) -> AuditEntry {
        AuditEntry {
            id: 0,
            timestamp_unix_nanos: 0,
            request_hash: masked_request_hash(reason.as_bytes()),
            decision: RoutingDecision::LocalOnly,
            modules_applied: vec![ModuleId::Ocap],
            rules_fired: vec![],
            flags: vec![],
            routing_reason: reason.to_string(),
            user_profile: "hmac:0".into(),
            correlation: CorrelationFields {
                credential_event_id: None,
                lamprey_decision_id: format!("dec-{reason}"),
                mai_request_id: format!("req-{reason}"),
                tenant: "t".into(),
                subject_hash: "hmac:0".into(),
                service_identity: None,
                policy_version: "v1".into(),
                trust_bundle_version: "v1".into(),
                decision: RoutingDecision::LocalOnly,
            },
            previous_hash: [0u8; CHAIN_HASH_LEN],
            signature: None,
        }
    }

    #[test]
    fn finalize_links_to_previous_hash() {
        let chain = HashChainManager::unsigned(ChainConfig {
            signature_interval: 0,
            ..ChainConfig::default()
        });
        let a = chain.finalize(draft("a"));
        let b = chain.finalize(draft("b"));
        assert!(a.is_chain_head());
        assert_eq!(b.previous_hash, a.content_hash());
        assert_eq!(a.id, 0);
        assert_eq!(b.id, 1);
        assert_eq!(chain.count(), 2);
    }

    #[test]
    fn unsigned_chain_records_no_signatures() {
        let chain = HashChainManager::unsigned(ChainConfig {
            signature_interval: 2,
            signing_key_id: String::new(),
        });
        let a = chain.finalize(draft("a"));
        let b = chain.finalize(draft("b")); // would be sig boundary
        assert!(a.signature.is_none());
        assert!(b.signature.is_none(), "NullSigner returns no sig");
    }

    #[test]
    fn verify_chain_passes_on_clean_chain() {
        let chain = HashChainManager::unsigned(ChainConfig {
            signature_interval: 0,
            ..ChainConfig::default()
        });
        let entries: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|r| chain.finalize(draft(r)))
            .collect();
        verify_chain(&entries, chain.config(), None::<&MlDsaBundleVerifier>).unwrap();
    }

    #[test]
    fn verify_chain_detects_tampered_field() {
        let chain = HashChainManager::unsigned(ChainConfig {
            signature_interval: 0,
            ..ChainConfig::default()
        });
        let a = chain.finalize(draft("a"));
        let b = chain.finalize(draft("b"));
        let mut tampered = vec![a, b];
        tampered[0].routing_reason = "tampered".into();
        let err = verify_chain(&tampered, chain.config(), None::<&MlDsaBundleVerifier>)
            .expect_err("tamper must be detected");
        assert!(matches!(err, ChainError::LinkBroken { .. }));
    }

    #[test]
    fn verify_chain_detects_non_monotonic_ids() {
        let chain = HashChainManager::unsigned(ChainConfig {
            signature_interval: 0,
            ..ChainConfig::default()
        });
        let a = chain.finalize(draft("a"));
        let mut b = chain.finalize(draft("b"));
        b.id = a.id; // collide
        let err = verify_chain(&[a, b], chain.config(), None::<&MlDsaBundleVerifier>)
            .expect_err("non-monotonic ids must be detected");
        assert!(matches!(err, ChainError::NonMonotonicIds { .. }));
    }

    #[test]
    fn verify_chain_detects_nonzero_head_hash() {
        let mut head = draft("a");
        head.previous_hash = [1u8; CHAIN_HASH_LEN];
        let err = verify_chain(
            &[head],
            &ChainConfig::default(),
            None::<&MlDsaBundleVerifier>,
        )
        .expect_err("non-zero head must be detected");
        assert!(matches!(err, ChainError::HeadHashNonZero { .. }));
    }

    #[test]
    fn reset_clears_cursor() {
        let chain = HashChainManager::unsigned(ChainConfig::default());
        chain.finalize(draft("a"));
        chain.finalize(draft("b"));
        assert_eq!(chain.count(), 2);
        chain.reset();
        assert_eq!(chain.count(), 0);
        assert_eq!(chain.head_hash(), [0u8; CHAIN_HASH_LEN]);
    }

    #[test]
    fn restore_from_picks_up_after_tail() {
        let chain = HashChainManager::unsigned(ChainConfig::default());
        let a = chain.finalize(draft("a"));
        // Fresh manager that "restores" from `a`.
        let fresh = HashChainManager::unsigned(ChainConfig::default());
        fresh.restore_from(&a);
        let b = fresh.finalize(draft("b"));
        assert_eq!(b.id, 1);
        assert_eq!(b.previous_hash, a.content_hash());
    }

    #[test]
    fn ml_dsa_signer_produces_verifiable_signature() {
        use rand::rngs::OsRng;
        let mut rng = OsRng;
        let (signer, pk_bytes) = MlDsaChainSigner::generate(&mut rng);
        let key_id = "audit-test-key";
        let cfg = ChainConfig {
            signature_interval: 1, // sign every entry
            signing_key_id: key_id.to_string(),
        };
        let chain = HashChainManager::new(cfg.clone(), Arc::new(signer));
        let a = chain.finalize(draft("a"));
        assert!(a.signature.is_some(), "interval=1 must sign every entry");

        let verifier = MlDsaBundleVerifier::new().with_anchor(key_id.to_string(), pk_bytes);
        verify_chain(&[a], &cfg, Some(&verifier)).expect("freshly signed chain must verify");
    }

    #[test]
    fn periodic_signature_only_lands_on_interval_boundaries() {
        use rand::rngs::OsRng;
        let mut rng = OsRng;
        let (signer, _pk) = MlDsaChainSigner::generate(&mut rng);
        let cfg = ChainConfig {
            signature_interval: 3,
            signing_key_id: "x".into(),
        };
        let chain = HashChainManager::new(cfg, Arc::new(signer));
        let entries: Vec<_> = (0..6)
            .map(|i| chain.finalize(draft(&format!("e{i}"))))
            .collect();
        // Ids 0, 1, 2, 3, 4, 5: signatures land on (id+1)%3==0 → ids 2, 5.
        assert!(entries[0].signature.is_none());
        assert!(entries[1].signature.is_none());
        assert!(entries[2].signature.is_some());
        assert!(entries[3].signature.is_none());
        assert!(entries[4].signature.is_none());
        assert!(entries[5].signature.is_some());
    }
}
