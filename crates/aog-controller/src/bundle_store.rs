//! Signed policy-bundle distribution: the artifact a gateway/node edge
//! fetches and verifies with the control-plane public key **alone**, and the
//! channel it is published on.
//!
//! A [`SignedBundle`] is the distributable form of a `PolicyBundle` spec —
//! ML-DSA-signed over its canonical payload (signature field excluded), exactly
//! like a `fabric-revocation` snapshot — so an edge verifies it **offline**,
//! even from removable media in an air-gap (doctrine I-8). [`EdgeBundleCache`]
//! is the edge's accept/reject decision: a bundle whose signature fails is
//! refused, and a **stale** bundle (version `<=` the one already applied) is
//! refused too — a validly-signed but replayed older bundle can never silently
//! downgrade enforcement (anti-rollback; doctrine I-3/I-4).
//!
//! [`BundleStore`] is the channel. [`OpenBaoBundleStore`] publishes to
//! `kv/data/policy-bundles/<name>` — the poll path established for edge
//! caches; [`MemBundleStore`] is its in-memory test double.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use fabric_contracts::Signature;
use fabric_crypto::providers::MlDsa87Verifier;
use fabric_crypto::{Signer, Verifier};
use fabric_proof::canonical_hash;
use wsf_bridge::OpenBaoAuth;

use aog_estate::{PolicyMode, PolicyRule};

use crate::runtime::ReconcileError;

/// The distributable, signed form of a `PolicyBundle` — what an edge fetches
/// from the channel and verifies with the control-plane public key alone.
/// Signed over its canonical payload with the `signature` field cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedBundle {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub mode: PolicyMode,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
    pub signature: Signature,
}

/// BLAKE3-32 over the canonical payload, `signature` removed — the exact bytes
/// signed and verified.
fn signing_hash(bundle: &SignedBundle) -> Result<[u8; 32], ReconcileError> {
    let mut v = serde_json::to_value(bundle).map_err(|e| ReconcileError(e.to_string()))?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    canonical_hash(&v).map_err(|e| ReconcileError(e.to_string()))
}

/// Sign a policy bundle into its distributable form.
///
/// # Errors
/// [`ReconcileError`] if canonical serialization or the signer fails.
pub fn sign_bundle(
    name: impl Into<String>,
    version: u32,
    mode: PolicyMode,
    rules: Vec<PolicyRule>,
    signer: &dyn Signer,
) -> Result<SignedBundle, ReconcileError> {
    let mut bundle = SignedBundle {
        name: name.into(),
        version,
        mode,
        rules,
        signature: Signature {
            alg: signer.algorithm().to_owned(),
            key_id: signer.key_id().to_owned(),
            value: String::new(),
        },
    };
    let hash = signing_hash(&bundle)?;
    let sig = signer
        .sign(&hash)
        .map_err(|e| ReconcileError(e.to_string()))?;
    bundle.signature.value = hex::encode(sig);
    Ok(bundle)
}

/// Verify a signed bundle's signature under `public_key` alone — the check an
/// offline edge runs. Wrong key, tampered content, or a malformed signature all
/// fail closed.
///
/// # Errors
/// [`ReconcileError`] if the signature is malformed or does not verify.
pub fn verify_bundle(
    bundle: &SignedBundle,
    verifier: &dyn Verifier,
    public_key: &[u8],
) -> Result<(), ReconcileError> {
    let hash = signing_hash(bundle)?;
    let sig = hex::decode(&bundle.signature.value)
        .map_err(|_| ReconcileError("bundle signature is not valid hex".to_owned()))?;
    match verifier.verify(&hash, &sig, public_key) {
        Ok(true) => Ok(()),
        _ => Err(ReconcileError(
            "bundle signature failed verification".to_owned(),
        )),
    }
}

/// Why an edge refused a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleReject {
    /// The signature did not verify under the control-plane public key.
    BadSignature,
    /// A validly-signed but stale bundle: its version does not exceed the one
    /// already applied. Refused so a replay cannot downgrade enforcement.
    Stale { applied: u32, offered: u32 },
}

impl std::fmt::Display for BundleReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => write!(f, "bundle signature failed verification"),
            Self::Stale { applied, offered } => {
                write!(f, "stale bundle v{offered} <= applied v{applied}")
            }
        }
    }
}

/// The edge's view of the channel: the signed bundle currently applied per
/// name, and the accept/reject decision a node makes when one arrives. The
/// public key is the sole trust input — no control-plane contact required.
pub struct EdgeBundleCache {
    public_key: Vec<u8>,
    verifier: MlDsa87Verifier,
    applied: HashMap<String, SignedBundle>,
}

impl EdgeBundleCache {
    /// A cache trusting `public_key` (the control-plane bundle-signing key).
    #[must_use]
    pub fn new(public_key: Vec<u8>) -> Self {
        Self {
            public_key,
            verifier: MlDsa87Verifier,
            applied: HashMap::new(),
        }
    }

    /// The bundle currently applied for `name`, if any.
    #[must_use]
    pub fn applied(&self, name: &str) -> Option<&SignedBundle> {
        self.applied.get(name)
    }

    /// Accept `bundle` if its signature verifies and its version advances the
    /// applied one; otherwise refuse (bad signature, or stale/replayed).
    ///
    /// # Errors
    /// [`BundleReject`] when the signature fails or the version is not newer.
    pub fn accept(&mut self, bundle: SignedBundle) -> Result<(), BundleReject> {
        if verify_bundle(&bundle, &self.verifier, &self.public_key).is_err() {
            return Err(BundleReject::BadSignature);
        }
        if let Some(current) = self.applied.get(&bundle.name)
            && bundle.version <= current.version
        {
            return Err(BundleReject::Stale {
                applied: current.version,
                offered: bundle.version,
            });
        }
        self.applied.insert(bundle.name.clone(), bundle);
        Ok(())
    }
}

/// The distribution channel a signed bundle is published on and an edge polls.
/// Production is [`OpenBaoBundleStore`]; tests use [`MemBundleStore`].
pub trait BundleStore: Send + Sync {
    /// Publish (overwrite) the signed bundle for `bundle.name`.
    fn publish(
        &self,
        bundle: &SignedBundle,
    ) -> impl Future<Output = Result<(), ReconcileError>> + Send;

    /// Fetch the currently-published signed bundle for `name`, if any.
    fn fetch(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<SignedBundle>, ReconcileError>> + Send;

    /// Retract the published bundle for `name`. Already absent = success.
    fn retract(&self, name: &str) -> impl Future<Output = Result<(), ReconcileError>> + Send;
}

/// In-memory [`BundleStore`] for tests.
#[derive(Default)]
pub struct MemBundleStore {
    map: Mutex<HashMap<String, SignedBundle>>,
}

impl MemBundleStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BundleStore for MemBundleStore {
    fn publish(
        &self,
        bundle: &SignedBundle,
    ) -> impl Future<Output = Result<(), ReconcileError>> + Send {
        let bundle = bundle.clone();
        async move {
            self.map
                .lock()
                .map_err(|_| ReconcileError("mem bundle store poisoned".to_owned()))?
                .insert(bundle.name.clone(), bundle);
            Ok(())
        }
    }

    fn fetch(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<SignedBundle>, ReconcileError>> + Send {
        let name = name.to_owned();
        async move {
            Ok(self
                .map
                .lock()
                .map_err(|_| ReconcileError("mem bundle store poisoned".to_owned()))?
                .get(&name)
                .cloned())
        }
    }

    fn retract(&self, name: &str) -> impl Future<Output = Result<(), ReconcileError>> + Send {
        let name = name.to_owned();
        async move {
            self.map
                .lock()
                .map_err(|_| ReconcileError("mem bundle store poisoned".to_owned()))?
                .remove(&name);
            Ok(())
        }
    }
}

/// OpenBao KV-v2 [`BundleStore`]: signed bundles live at
/// `<mount>/data/policy-bundles/<name>`. Auth rides the same AppRole login the
/// Transit admin uses.
pub struct OpenBaoBundleStore {
    openbao: OpenBaoAuth,
    http: Client,
    mount: String,
}

impl OpenBaoBundleStore {
    /// A store over the default `kv` KV-v2 mount.
    ///
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn new(openbao: OpenBaoAuth) -> Result<Self, ReconcileError> {
        Self::with_mount(openbao, "kv")
    }

    /// A store over a named KV-v2 mount.
    ///
    /// # Errors
    /// [`ReconcileError`] if the HTTP client cannot be built.
    pub fn with_mount(
        openbao: OpenBaoAuth,
        mount: impl Into<String>,
    ) -> Result<Self, ReconcileError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ReconcileError(e.to_string()))?;
        Ok(Self {
            openbao,
            http,
            mount: mount.into(),
        })
    }

    fn data_path(&self, name: &str) -> String {
        format!("{}/data/policy-bundles/{name}", self.mount)
    }

    async fn login(&self) -> Result<String, ReconcileError> {
        self.openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, ReconcileError> {
        let token = self.login().await?;
        let url = format!("{}/v1/{path}", self.openbao.address());
        let mut rb = self
            .http
            .request(method, url)
            .header("X-Vault-Token", token);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        rb.send().await.map_err(|e| ReconcileError(e.to_string()))
    }
}

impl BundleStore for OpenBaoBundleStore {
    fn publish(
        &self,
        bundle: &SignedBundle,
    ) -> impl Future<Output = Result<(), ReconcileError>> + Send {
        let bundle = bundle.clone();
        async move {
            let path = self.data_path(&bundle.name);
            let resp = self
                .call(Method::POST, &path, Some(json!({ "data": bundle })))
                .await?;
            if resp.status().is_success() {
                Ok(())
            } else {
                Err(ReconcileError(format!("bundle publish: {}", resp.status())))
            }
        }
    }

    fn fetch(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<SignedBundle>, ReconcileError>> + Send {
        let name = name.to_owned();
        async move {
            let resp = self.call(Method::GET, &self.data_path(&name), None).await?;
            match resp.status() {
                StatusCode::NOT_FOUND => Ok(None),
                s if s.is_success() => {
                    let v: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ReconcileError(e.to_string()))?;
                    let inner = &v["data"]["data"];
                    if inner.is_null() {
                        return Ok(None);
                    }
                    let bundle: SignedBundle = serde_json::from_value(inner.clone())
                        .map_err(|e| ReconcileError(e.to_string()))?;
                    Ok(Some(bundle))
                }
                s => Err(ReconcileError(format!("bundle fetch: {s}"))),
            }
        }
    }

    fn retract(&self, name: &str) -> impl Future<Output = Result<(), ReconcileError>> + Send {
        let name = name.to_owned();
        async move {
            // KV-v2 soft-delete of the latest version: the poll path then 404s.
            let resp = self
                .call(Method::DELETE, &self.data_path(&name), None)
                .await?;
            if resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND {
                Ok(())
            } else {
                Err(ReconcileError(format!("bundle retract: {}", resp.status())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_contracts::RoutingDecision;
    use fabric_crypto::providers::RustCryptoMlDsa87;

    fn signer(id: &str) -> RustCryptoMlDsa87 {
        RustCryptoMlDsa87::generate(id).unwrap()
    }

    fn rule(name: &str) -> PolicyRule {
        PolicyRule {
            name: name.to_owned(),
            effect: RoutingDecision::Deny,
            when: String::new(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrips_and_tamper_fails() {
        let s = signer("cp-key");
        let bundle = sign_bundle(
            "baseline",
            1,
            PolicyMode::Enforce,
            vec![rule("no-egress")],
            &s,
        )
        .unwrap();

        // Verifies under the right key.
        verify_bundle(&bundle, &MlDsa87Verifier, s.public_key()).unwrap();

        // Wrong key fails closed.
        let other = signer("other-key");
        assert!(verify_bundle(&bundle, &MlDsa87Verifier, other.public_key()).is_err());

        // Tampered content (signature carried over) fails closed.
        let mut tampered = bundle.clone();
        tampered.rules.push(rule("smuggled-allow"));
        assert!(verify_bundle(&tampered, &MlDsa87Verifier, s.public_key()).is_err());
    }

    #[test]
    fn edge_accepts_forward_and_refuses_stale_and_bad_signature() {
        let s = signer("cp-key");
        let mut edge = EdgeBundleCache::new(s.public_key().to_vec());

        let v1 = sign_bundle("main", 1, PolicyMode::Enforce, vec![], &s).unwrap();
        let v2 = sign_bundle("main", 2, PolicyMode::Enforce, vec![rule("r")], &s).unwrap();

        edge.accept(v1.clone()).unwrap();
        assert_eq!(edge.applied("main").unwrap().version, 1);
        edge.accept(v2).unwrap();
        assert_eq!(edge.applied("main").unwrap().version, 2);

        // A replayed older-but-validly-signed bundle is refused (anti-rollback).
        assert_eq!(
            edge.accept(v1),
            Err(BundleReject::Stale {
                applied: 2,
                offered: 1
            })
        );
        // Still on v2 — enforcement was not downgraded.
        assert_eq!(edge.applied("main").unwrap().version, 2);

        // A forged signature is refused.
        let forged = signer("attacker");
        let bad = sign_bundle("main", 3, PolicyMode::Shadow, vec![], &forged).unwrap();
        assert_eq!(edge.accept(bad), Err(BundleReject::BadSignature));
        assert_eq!(edge.applied("main").unwrap().version, 2);
    }

    #[tokio::test]
    async fn mem_store_publishes_fetches_and_retracts() {
        let s = signer("cp-key");
        let store = MemBundleStore::new();
        assert!(store.fetch("main").await.unwrap().is_none());

        let b = sign_bundle("main", 1, PolicyMode::Enforce, vec![], &s).unwrap();
        store.publish(&b).await.unwrap();
        assert_eq!(store.fetch("main").await.unwrap(), Some(b));

        store.retract("main").await.unwrap();
        assert!(store.fetch("main").await.unwrap().is_none());
        // Retracting an absent bundle is convergence, not an error.
        store.retract("main").await.unwrap();
    }
}
