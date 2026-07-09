//! The VirtualKey controller: a declared `VirtualKey` becomes a resolvable
//! entry at the gateway's key-resolution path, so the gateway (G1) turns the
//! presented key into a verified, scoped, in-budget trust token — and a change
//! to the key's capability is reflected on the gateway's next request, with no
//! restart.
//!
//! The gateway resolves a key by reading `<prefix>/<sha256(key)>` from OpenBao
//! KV and verifying the `token` it finds against the trust anchor. This
//! controller writes that entry: it mints a token from the `Capability` the
//! `VirtualKey` names (scope + budget + ttl), signs it with the anchor, and
//! puts it at the key's path. It owns a finalizer so the entry is retracted
//! **before** the estate object is collected — a deleted key must stop
//! resolving (fail-closed, I-4), never linger. The kernel models the presented
//! key by the object's name; a secret-key indirection is a Phase-W concern.

use std::future::Future;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};

use fabric_contracts::{Attenuation, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;
use wsf_bridge::OpenBaoAuth;

use aog_estate::{CapabilitySpec, Kind, Phase, ResourceObject, VirtualKey, VirtualKeyStatus};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The finalizer this controller owns on every `VirtualKey`: the estate object
/// may not vanish until its gateway resolution entry is retracted.
pub const VIRTUALKEY_FINALIZER: &str = "loom.aog/virtualkey-kv";

/// The stable token id a virtual key resolves to (also its revocation subject).
fn token_id(tenant: &str, name: &str) -> String {
    format!("vk:{tenant}:{name}")
}

/// VirtualKey resolution controller. Run it on a `"VirtualKey/"` informer.
#[derive(Clone)]
pub struct VirtualKeyController {
    client: EstateClient,
    openbao: Arc<OpenBaoAuth>,
    vk_prefix: String,
    signer: Arc<dyn Signer>,
}

impl VirtualKeyController {
    /// `vk_prefix` must equal the gateway's `virtual_key_kv_prefix`
    /// (e.g. `kv/data/aog/virtual-keys`); `signer` is the trust anchor whose
    /// public key the gateway verifies resolved tokens against.
    #[must_use]
    pub fn new(
        client: EstateClient,
        openbao: Arc<OpenBaoAuth>,
        vk_prefix: impl Into<String>,
        signer: Arc<dyn Signer>,
    ) -> Self {
        Self {
            client,
            openbao,
            vk_prefix: vk_prefix.into(),
            signer,
        }
    }

    /// The KV path the gateway reads for `name` — `<prefix>/<sha256(name)>`.
    fn key_path(&self, name: &str) -> String {
        let hash = hex::encode(Sha256::digest(name.as_bytes()));
        format!("{}/{hash}", self.vk_prefix)
    }

    /// The token currently published for `name`, if any.
    async fn published(
        &self,
        vault: &str,
        name: &str,
    ) -> Result<Option<TrustToken>, ReconcileError> {
        match self.openbao.get_kv_data(vault, &self.key_path(name)).await {
            Ok(data) => match data.get("token") {
                Some(value) => Ok(Some(
                    serde_json::from_value(value.clone())
                        .map_err(|e| ReconcileError(e.to_string()))?,
                )),
                None => Ok(None),
            },
            Err(wsf_bridge::OpenBaoError::NotFound(_)) => Ok(None),
            Err(e) => Err(ReconcileError(e.to_string())),
        }
    }

    /// Does `published` already resolve `cap` for `tenant` (scope, budget, and a
    /// valid anchor signature)?
    fn in_sync(&self, published: &TrustToken, cap: &CapabilitySpec, tenant: &str) -> bool {
        published.tenant_id == tenant
            && published.allowed_routes == cap.allowed_routes
            && published.allowed_models == cap.allowed_models
            && published.max_data_classification == cap.max_classification
            && published.budget.as_ref() == Some(&cap.budget)
            && published.attenuation.caveats == cap.caveats
            && fabric_token::verify(published, &MlDsa87Verifier, self.signer.public_key()).is_ok()
    }

    /// Mint a trust token carrying `cap`'s scope + budget for `tenant`.
    fn mint(
        &self,
        id: String,
        tenant: &str,
        cap: &CapabilitySpec,
    ) -> Result<TrustToken, ReconcileError> {
        let now = Utc::now();
        let ttl = Duration::seconds(i64::try_from(cap.ttl_seconds).unwrap_or(i64::MAX));
        let token = TrustToken {
            token_id: id.clone(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + ttl).to_rfc3339(),
            issuer: "aog-controller".to_owned(),
            trust_bundle_version: "loom".to_owned(),
            tenant_id: tenant.to_owned(),
            subject_id: None,
            subject_hash: id,
            service_identity: None,
            identity_id: None,
            roles: vec![],
            compliance_scopes: vec![],
            allowed_routes: cap.allowed_routes.clone(),
            allowed_models: cap.allowed_models.clone(),
            max_data_classification: cap.max_classification,
            country: None,
            person_type: None,
            offline_mode: false,
            revocation_status: RevocationStatus::Valid,
            budget: Some(cap.budget.clone()),
            attenuation: Attenuation {
                parent_id: None,
                caveats: cap.caveats.clone(),
            },
            signature: Signature {
                alg: String::new(),
                key_id: String::new(),
                value: String::new(),
            },
        };
        fabric_token::issue(token, self.signer.as_ref()).map_err(|e| ReconcileError(e.to_string()))
    }

    /// Retract the gateway resolution entry for `name` (soft-delete the KV
    /// record; the gateway then returns `UnknownKey`).
    async fn retract(&self, vault: &str, name: &str) -> Result<(), ReconcileError> {
        self.openbao
            .delete_kv(vault, &self.key_path(name))
            .await
            .map_err(|e| ReconcileError(e.to_string()))
    }

    /// Reflect convergence in status; write only on change.
    async fn set_status(
        &self,
        vkey: VirtualKey,
        phase: Phase,
        resolved_token: Option<String>,
    ) -> Result<Action, ReconcileError> {
        let desired = VirtualKeyStatus {
            phase,
            resolved_token,
        };
        if vkey.status.as_ref() != Some(&desired) {
            let mut converged = vkey;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::VirtualKey(converged))
                .await?;
        }
        Ok(Action::Done)
    }

    async fn reconcile_vkey(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::VirtualKey(vkey)) =
            self.client.get(Kind::VirtualKey, name).await?
        else {
            return Ok(Action::Done);
        };

        // Terminating: retract the gateway entry, then release our finalizer.
        if vkey.metadata.deletion_timestamp.is_some() {
            if !vkey
                .metadata
                .finalizers
                .iter()
                .any(|f| f == VIRTUALKEY_FINALIZER)
            {
                return Ok(Action::Done); // our leg already released
            }
            let vault = self
                .openbao
                .login()
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            self.retract(&vault, name).await?;
            let mut released = vkey;
            released
                .metadata
                .finalizers
                .retain(|f| f != VIRTUALKEY_FINALIZER);
            self.client
                .update(ResourceObject::VirtualKey(released))
                .await?;
            return Ok(Action::Done);
        }

        // Live: guard with our finalizer first (the update wakes us again).
        if !vkey
            .metadata
            .finalizers
            .iter()
            .any(|f| f == VIRTUALKEY_FINALIZER)
        {
            let mut guarded = vkey;
            guarded
                .metadata
                .finalizers
                .push(VIRTUALKEY_FINALIZER.to_owned());
            self.client
                .update(ResourceObject::VirtualKey(guarded))
                .await?;
            return Ok(Action::Done);
        }

        let vault = self
            .openbao
            .login()
            .await
            .map_err(|e| ReconcileError(e.to_string()))?;

        // The capability the key resolves to must exist and be live; otherwise
        // the key must not resolve to a stale token (fail-closed).
        let cap = match self
            .client
            .get(Kind::Capability, &vkey.spec.capability)
            .await?
        {
            Some(ResourceObject::Capability(c)) if c.metadata.deletion_timestamp.is_none() => c,
            _ => {
                self.retract(&vault, name).await?;
                return self.set_status(vkey, Phase::Degraded, None).await;
            }
        };

        // Mint + publish only on drift (absent, scope changed, or tampered).
        let id = token_id(&vkey.spec.tenant, name);
        let published = self.published(&vault, name).await?;
        let in_sync = published
            .as_ref()
            .is_some_and(|t| t.token_id == id && self.in_sync(t, &cap.spec, &vkey.spec.tenant));
        if !in_sync {
            let token = self.mint(id.clone(), &vkey.spec.tenant, &cap.spec)?;
            self.openbao
                .put_kv_data(&vault, &self.key_path(name), json!({ "token": token }))
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
        }

        self.set_status(vkey, Phase::Ready, Some(id)).await
    }
}

impl Reconciler for VirtualKeyController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::VirtualKey, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_vkey(name).await
        }
    }
}
