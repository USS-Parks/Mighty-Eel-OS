//! The ProviderPool controller: fold each pool's live provider/model
//! health into its schedulable set (`status.healthy`), so the scheduler
//! (Phase S) only ever places on a currently-reachable endpoint.
//!
//! Level-triggered and idempotent, and driven by a resync heartbeat
//! ([`Controller::with_resync`](crate::Controller::with_resync)) so health is
//! re-checked on a cadence even when desired-state does not change — an
//! endpoint that goes unhealthy drops out of the schedulable set within that
//! SLO, and one that recovers rejoins. A pool with endpoints but none healthy
//! is `Degraded` (nothing schedulable), not silently `Ready`.

use std::future::Future;
use std::sync::Arc;

use aog_estate::{Kind, Phase, ProviderPoolStatus, ResourceObject};

use crate::health::HealthProbe;
use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// ProviderPool health controller. Run it on a `"ProviderPool/"` informer with
/// a resync heartbeat.
pub struct ProviderPoolController<P: HealthProbe> {
    client: EstateClient,
    probe: Arc<P>,
}

// Hand-written so the controller clones without requiring `P: Clone` (it holds
// only an `Arc<P>`).
impl<P: HealthProbe> Clone for ProviderPoolController<P> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            probe: Arc::clone(&self.probe),
        }
    }
}

impl<P: HealthProbe> ProviderPoolController<P> {
    #[must_use]
    pub fn new(client: EstateClient, probe: Arc<P>) -> Self {
        Self { client, probe }
    }

    async fn reconcile_pool(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::ProviderPool(pool)) =
            self.client.get(Kind::ProviderPool, name).await?
        else {
            return Ok(Action::Done);
        };
        // Terminating: no external state to unwind (health is observed, not
        // provisioned); GC collects the estate object.
        if pool.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done);
        }

        // Probe every endpoint; the healthy ones are the schedulable set.
        let mut healthy = Vec::new();
        for endpoint in &pool.spec.endpoints {
            if self.probe.healthy(&pool.spec.provider, endpoint).await {
                healthy.push(endpoint.model.clone());
            }
        }
        healthy.sort();
        healthy.dedup();

        let phase = if !pool.spec.endpoints.is_empty() && healthy.is_empty() {
            Phase::Degraded
        } else {
            Phase::Ready
        };
        let desired = ProviderPoolStatus { phase, healthy };
        if pool.status.as_ref() != Some(&desired) {
            let mut converged = pool;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::ProviderPool(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl<P: HealthProbe + 'static> Reconciler for ProviderPoolController<P> {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::ProviderPool, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_pool(name).await
        }
    }
}
