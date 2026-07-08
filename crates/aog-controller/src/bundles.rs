//! R6 — the PolicyBundle controller: a declared `PolicyBundle` is signed and
//! published to the channel every gateway/node edge polls, and its status
//! records where it reached.
//!
//! Level-triggered and idempotent: it (re)signs and publishes only when the
//! channel is absent, content-drifted, or tampered — never on every pass. It
//! **never regresses** the channel: a spec whose version is behind the live
//! artifact is a stale declaration and is refused (`Degraded`), not shipped, so
//! a mistaken or replayed edit cannot downgrade estate-wide enforcement
//! (doctrine I-4). The bundle carries a monotonic version; anti-rollback at the
//! edge ([`EdgeBundleCache`](crate::bundle_store::EdgeBundleCache)) is the
//! second layer — rollback is roll-forward to a new signed version, and every
//! prior signed artifact stays independently verifiable.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;

use aog_estate::{Kind, Phase, PolicyBundle, PolicyBundleStatus, ResourceObject, WorkloadKind};

use crate::bundle_store::{BundleStore, sign_bundle, verify_bundle};
use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// PolicyBundle distribution controller. Run it on a `"PolicyBundle/"` informer.
pub struct PolicyBundleController<S: BundleStore> {
    client: EstateClient,
    store: Arc<S>,
    signer: Arc<dyn Signer>,
}

// Hand-written so the controller clones without requiring `S: Clone` (it holds
// only an `Arc<S>`).
impl<S: BundleStore> Clone for PolicyBundleController<S> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            store: Arc::clone(&self.store),
            signer: Arc::clone(&self.signer),
        }
    }
}

impl<S: BundleStore> PolicyBundleController<S> {
    #[must_use]
    pub fn new(client: EstateClient, store: Arc<S>, signer: Arc<dyn Signer>) -> Self {
        Self {
            client,
            store,
            signer,
        }
    }

    /// The gateways and nodes the channel serves — recorded in `distributed_to`
    /// so the estate shows, per bundle, which edges it is published for.
    async fn distribution_targets(&self) -> Result<Vec<String>, ReconcileError> {
        let mut targets = BTreeSet::new();
        for object in self.client.list(Kind::Node).await? {
            targets.insert(object.name().to_owned());
        }
        for object in self.client.list(Kind::Workload).await? {
            if let ResourceObject::Workload(workload) = &object
                && workload.spec.workload_kind == WorkloadKind::Gateway
            {
                targets.insert(object.name().to_owned());
            }
        }
        Ok(targets.into_iter().collect())
    }

    async fn reconcile_bundle(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::PolicyBundle(bundle)) =
            self.client.get(Kind::PolicyBundle, name).await?
        else {
            return Ok(Action::Done);
        };

        // Terminating: pull the artifact from the channel; the estate object is
        // R2 GC's to collect.
        if bundle.metadata.deletion_timestamp.is_some() {
            self.store.retract(name).await?;
            return Ok(Action::Done);
        }

        let published = self.store.fetch(name).await?;
        let public_key = self.signer.public_key().to_vec();

        // Never downgrade the channel to a stale declaration (I-4).
        if published
            .as_ref()
            .is_some_and(|p| p.version > bundle.spec.version)
        {
            return self.mark(bundle, Phase::Degraded, Vec::new()).await;
        }

        // Publish only on real drift: absent, content-changed, or tampered.
        let in_sync = published.as_ref().is_some_and(|p| {
            p.version == bundle.spec.version
                && p.mode == bundle.spec.mode
                && p.rules == bundle.spec.rules
                && verify_bundle(p, &MlDsa87Verifier, &public_key).is_ok()
        });
        if !in_sync {
            let signed = sign_bundle(
                name,
                bundle.spec.version,
                bundle.spec.mode,
                bundle.spec.rules.clone(),
                self.signer.as_ref(),
            )?;
            self.store.publish(&signed).await?;
        }

        let targets = self.distribution_targets().await?;
        self.mark(bundle, Phase::Ready, targets).await
    }

    /// Reflect convergence in status; write only on change.
    async fn mark(
        &self,
        bundle: PolicyBundle,
        phase: Phase,
        distributed_to: Vec<String>,
    ) -> Result<Action, ReconcileError> {
        let desired = PolicyBundleStatus {
            phase,
            distributed_to,
        };
        if bundle.status.as_ref() != Some(&desired) {
            let mut converged = bundle;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::PolicyBundle(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl<S: BundleStore + 'static> Reconciler for PolicyBundleController<S> {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::PolicyBundle, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_bundle(name).await
        }
    }
}
