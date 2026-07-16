//! O6 — ToolGrant orchestration: the declarative `ToolGrant`s in the estate are
//! compiled into a single **signed, versioned active-grant set** and published on
//! the channel every `aog-toolproxy` edge polls. A proxy verifies the set with
//! the control-plane public key alone ([`EdgeGrantCache`]) and consults it
//! **per call** — so revoking a grant (deleting it, or its owning mission) drops
//! the tool from the next published set and the proxy denies the tool's next
//! call, halting it mid-run on every proxy at once.
//!
//! This is the tool-access sibling of the policy-bundle distribution: the same
//! ML-DSA-over-canonical-payload signing, the same offline-verifiable artifact,
//! and the same anti-rollback (a validly-signed but older set is refused, so a
//! replay can never resurrect a revoked grant). The per-call credential itself is
//! still minted at the proxy boundary (T2, `CredentialMinter`); O6 governs *which*
//! tools may be minted for at all.

use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use fabric_contracts::Signature;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;

use aog_estate::{Kind, ResourceObject};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// One granted tool in the published set: the tool and the systems it may reach
/// (empty = unrestricted).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GrantEntry {
    /// Immutable tenant partition for this authority entry.
    #[serde(default)]
    pub tenant_id: String,
    pub tool: String,
    pub systems: Vec<String>,
}

/// The signed, versioned set of tool grants currently in force — what a proxy
/// polls and verifies with the control-plane public key alone. Signed over its
/// canonical payload with the `signature` field cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGrantSet {
    pub version: u32,
    pub grants: Vec<GrantEntry>,
    pub signature: Signature,
}

/// BLAKE3-32 over the canonical payload with `signature` removed — the exact
/// bytes signed and verified.
fn signing_hash(set: &SignedGrantSet) -> Result<[u8; 32], ReconcileError> {
    let mut v = serde_json::to_value(set).map_err(|e| ReconcileError(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| ReconcileError(e.to_string()))
}

/// Sign an active-grant set into its distributable form. Entries are sorted first
/// so the same logical set always yields the same bytes (deterministic).
///
/// # Errors
/// [`ReconcileError`] if canonical serialization or the signer fails.
pub fn sign_grants(
    version: u32,
    mut grants: Vec<GrantEntry>,
    signer: &dyn Signer,
) -> Result<SignedGrantSet, ReconcileError> {
    grants.sort();
    grants.dedup();
    let mut set = SignedGrantSet {
        version,
        grants,
        signature: Signature {
            alg: signer.algorithm().to_owned(),
            key_id: signer.key_id().to_owned(),
            value: String::new(),
        },
    };
    let hash = signing_hash(&set)?;
    set.signature.value = hex::encode(
        signer
            .sign(&hash)
            .map_err(|e| ReconcileError(e.to_string()))?,
    );
    Ok(set)
}

/// Verify a signed grant set under `public_key` alone — the check an offline proxy
/// runs. Wrong key, tampered content, or a malformed signature all fail closed.
///
/// # Errors
/// [`ReconcileError`] if the signature is malformed or does not verify.
pub fn verify_grants(
    set: &SignedGrantSet,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), ReconcileError> {
    let hash = signing_hash(set)?;
    let sig = hex::decode(&set.signature.value)
        .map_err(|_| ReconcileError("grant-set signature is not valid hex".to_owned()))?;
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(ReconcileError(
            "grant-set signature failed verification".to_owned(),
        )),
    }
}

/// Why a proxy refused a published grant set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantReject {
    /// The signature did not verify under the control-plane public key.
    BadSignature,
    /// A validly-signed but stale set: its version does not exceed the applied
    /// one. Refused so a replay cannot resurrect a revoked grant.
    Stale { applied: u32, offered: u32 },
}

impl std::fmt::Display for GrantReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => write!(f, "grant-set signature failed verification"),
            Self::Stale { applied, offered } => {
                write!(f, "stale grant set v{offered} <= applied v{applied}")
            }
        }
    }
}

/// The proxy edge's enforcement view: the active grant set it has applied,
/// verified by the control-plane public key alone. `allows` is the per-call
/// check — a tool absent from the applied set is denied, so a revoked grant
/// halts the tool's next call.
pub struct EdgeGrantCache {
    public_key: Vec<u8>,
    verifier: MlDsa87Verifier,
    applied: Option<SignedGrantSet>,
}

impl EdgeGrantCache {
    /// A cache trusting `public_key` (the control-plane grant-signing key).
    #[must_use]
    pub fn new(public_key: Vec<u8>) -> Self {
        Self {
            public_key,
            verifier: MlDsa87Verifier,
            applied: None,
        }
    }

    /// The version currently applied, if any.
    #[must_use]
    pub fn version(&self) -> Option<u32> {
        self.applied.as_ref().map(|s| s.version)
    }

    /// Accept `set` if its signature verifies and its version advances the applied
    /// one; otherwise refuse (bad signature, or stale/replayed).
    ///
    /// # Errors
    /// [`GrantReject`] when the signature fails or the version is not newer.
    pub fn accept(&mut self, set: SignedGrantSet) -> Result<(), GrantReject> {
        if verify_grants(&set, &self.verifier, &self.public_key).is_err() {
            return Err(GrantReject::BadSignature);
        }
        if let Some(current) = &self.applied
            && set.version <= current.version
        {
            return Err(GrantReject::Stale {
                applied: current.version,
                offered: set.version,
            });
        }
        self.applied = Some(set);
        Ok(())
    }

    /// Whether `tool` is granted right now (present in the applied set).
    #[must_use]
    pub fn allows(&self, tenant_id: &str, tool: &str) -> bool {
        self.applied.as_ref().is_some_and(|s| {
            s.grants
                .iter()
                .any(|g| g.tenant_id == tenant_id && g.tool == tool)
        })
    }

    /// Whether `tool` may reach `system`: the tool is granted and either the grant
    /// is system-unrestricted or names `system`.
    #[must_use]
    pub fn allows_system(&self, tenant_id: &str, tool: &str, system: &str) -> bool {
        self.applied.as_ref().is_some_and(|set| {
            set.grants.iter().any(|g| {
                g.tenant_id == tenant_id
                    && g.tool == tool
                    && (g.systems.is_empty() || g.systems.iter().any(|s| s == system))
            })
        })
    }
}

/// The channel a signed grant set is published on and a proxy polls. Production
/// wiring publishes to the same OpenBao poll path the bundle channel uses;
/// [`MemGrantStore`] is the in-memory test/shared double.
pub trait GrantStore: Send + Sync {
    /// Publish (overwrite) the active grant set.
    fn publish(
        &self,
        set: &SignedGrantSet,
    ) -> impl Future<Output = Result<(), ReconcileError>> + Send;

    /// Fetch the currently-published grant set, if any.
    fn fetch(&self) -> impl Future<Output = Result<Option<SignedGrantSet>, ReconcileError>> + Send;
}

/// In-memory grant channel — shared by every proxy edge in a test, the way the
/// real channel is shared across the estate.
#[derive(Default)]
pub struct MemGrantStore {
    inner: Mutex<Option<SignedGrantSet>>,
}

impl MemGrantStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl GrantStore for MemGrantStore {
    fn publish(
        &self,
        set: &SignedGrantSet,
    ) -> impl Future<Output = Result<(), ReconcileError>> + Send {
        let set = set.clone();
        async move {
            *self.inner.lock().expect("grant store poisoned") = Some(set);
            Ok(())
        }
    }

    fn fetch(&self) -> impl Future<Output = Result<Option<SignedGrantSet>, ReconcileError>> + Send {
        let current = self.inner.lock().expect("grant store poisoned").clone();
        async move { Ok(current) }
    }
}

/// Compiles the estate's live `ToolGrant`s into the published active-grant set.
/// Run it on a `"ToolGrant/"` informer: any grant change recomputes and (when
/// changed) republishes the whole set with a monotonically advancing version.
pub struct ToolGrantController<S: GrantStore> {
    client: EstateClient,
    store: Arc<S>,
    signer: Arc<dyn Signer>,
}

// Hand-written so the controller clones without requiring `S: Clone`.
impl<S: GrantStore> Clone for ToolGrantController<S> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            store: Arc::clone(&self.store),
            signer: Arc::clone(&self.signer),
        }
    }
}

impl<S: GrantStore> ToolGrantController<S> {
    #[must_use]
    pub fn new(client: EstateClient, store: Arc<S>, signer: Arc<dyn Signer>) -> Self {
        Self {
            client,
            store,
            signer,
        }
    }

    /// The set of grants currently in force — every live (non-terminating)
    /// `ToolGrant`. A terminating or deleted grant is excluded, which is exactly
    /// how a revocation drops the tool.
    async fn active_grants(&self) -> Result<Vec<GrantEntry>, ReconcileError> {
        let mut grants = Vec::new();
        for object in self.client.list(Kind::ToolGrant).await? {
            if let ResourceObject::ToolGrant(grant) = object
                && grant.metadata.deletion_timestamp.is_none()
            {
                grants.push(GrantEntry {
                    tenant_id: grant.metadata.tenant.unwrap_or_default(),
                    tool: grant.spec.tool,
                    systems: grant.spec.systems,
                });
            }
        }
        grants.sort();
        grants.dedup();
        Ok(grants)
    }

    async fn reconcile_grants(&self) -> Result<Action, ReconcileError> {
        let desired = self.active_grants().await?;
        let published = self.store.fetch().await?;
        // Publish only on real change; advance the version so a proxy's
        // anti-rollback never mistakes a fresh set for a replay.
        let changed = published.as_ref().is_none_or(|p| p.grants != desired);
        if changed {
            let version = published.as_ref().map_or(1, |p| p.version + 1);
            let set = sign_grants(version, desired, self.signer.as_ref())?;
            self.store.publish(&set).await?;
        }
        Ok(Action::Done)
    }
}

impl<S: GrantStore + 'static> Reconciler for ToolGrantController<S> {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            // Any ToolGrant change recomputes the whole set; a non-grant key is
            // ignored.
            let Some((Kind::ToolGrant, _)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_grants().await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_crypto::providers::RustCryptoMlDsa87;

    fn signer() -> Arc<dyn Signer> {
        Arc::new(RustCryptoMlDsa87::generate("loom-o6-test").unwrap())
    }

    fn entry(tool: &str, systems: &[&str]) -> GrantEntry {
        GrantEntry {
            tenant_id: "tenant-a".into(),
            tool: tool.to_owned(),
            systems: systems.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn a_signed_set_verifies_and_a_proxy_allows_its_tools() {
        let s = signer();
        let set = sign_grants(
            1,
            vec![entry("search", &["crm"]), entry("calc", &[])],
            s.as_ref(),
        )
        .unwrap();
        let mut edge = EdgeGrantCache::new(s.public_key().to_vec());
        edge.accept(set).expect("valid set accepted");
        assert!(edge.allows("tenant-a", "search") && edge.allows("tenant-a", "calc"));
        assert!(
            !edge.allows("tenant-a", "delete_db"),
            "an ungranted tool is denied"
        );
        assert!(edge.allows_system("tenant-a", "search", "crm"));
        assert!(
            !edge.allows_system("tenant-a", "search", "prod"),
            "system scope enforced"
        );
        assert!(
            edge.allows_system("tenant-a", "calc", "anything"),
            "unrestricted grant reaches any system"
        );
    }

    #[test]
    fn identical_tool_names_do_not_cross_tenant_partitions() {
        let signer = RustCryptoMlDsa87::generate("grant-tenant-anchor").unwrap();
        let set = sign_grants(1, vec![entry("search", &["crm"])], &signer).unwrap();
        let mut edge = EdgeGrantCache::new(signer.public_key().to_vec());
        edge.accept(set).unwrap();
        assert!(edge.allows("tenant-a", "search"));
        assert!(edge.allows_system("tenant-a", "search", "crm"));
        assert!(!edge.allows("tenant-b", "search"));
        assert!(!edge.allows_system("tenant-b", "search", "crm"));
    }

    #[test]
    fn a_wrong_key_is_refused() {
        let set = sign_grants(1, vec![entry("search", &[])], signer().as_ref()).unwrap();
        let mut edge = EdgeGrantCache::new(signer().public_key().to_vec()); // different key
        assert_eq!(edge.accept(set), Err(GrantReject::BadSignature));
    }

    #[test]
    fn a_stale_replay_is_refused() {
        let s = signer();
        let mut edge = EdgeGrantCache::new(s.public_key().to_vec());
        let v2 = sign_grants(2, vec![entry("search", &[])], s.as_ref()).unwrap();
        edge.accept(v2).unwrap();
        let v1 = sign_grants(
            1,
            vec![entry("search", &[]), entry("calc", &[])],
            s.as_ref(),
        )
        .unwrap();
        assert_eq!(
            edge.accept(v1),
            Err(GrantReject::Stale {
                applied: 2,
                offered: 1
            }),
            "an older validly-signed set cannot resurrect a revoked grant"
        );
    }

    #[test]
    fn revocation_drops_the_tool_from_a_newer_set() {
        let s = signer();
        let mut edge = EdgeGrantCache::new(s.public_key().to_vec());
        edge.accept(
            sign_grants(
                1,
                vec![entry("search", &[]), entry("calc", &[])],
                s.as_ref(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(edge.allows("tenant-a", "calc"));
        // A newer set without calc = calc revoked.
        edge.accept(sign_grants(2, vec![entry("search", &[])], s.as_ref()).unwrap())
            .unwrap();
        assert!(edge.allows("tenant-a", "search") && !edge.allows("tenant-a", "calc"));
    }
}
