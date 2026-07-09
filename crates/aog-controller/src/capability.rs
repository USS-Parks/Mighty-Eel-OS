//! The Capability / budget controller.
//!
//! A `Capability` declares the scope + budget tokens are minted from. Its
//! budget is enforced through the **shared** spend ledger (X1): every replica
//! metering against the capability leases slices of one atomic record keyed
//! by [`spend_key`], so the cap holds across the estate — the F3 contract,
//! no longer per-process. The shared record is created lazily by the first
//! lease acquisition; this controller's job is lifecycle: a live, valid
//! capability is reported `Ready` (tokens may be resolved against it),
//! and a terminating one is left to the GC/finalizer machinery.

use std::future::Future;

use aog_estate::{CapabilityStatus, Kind, Phase, ResourceObject};

use crate::objects::{EstateClient, is_terminating, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The shared spend-record key a capability's budget is metered under —
/// what every replica's `LeasedSpendLedger` leases against.
#[must_use]
pub fn spend_key(capability: &str) -> String {
    format!("cap-{capability}")
}

/// Capability lifecycle controller. Run it on a `"Capability/"` informer.
#[derive(Clone)]
pub struct CapabilityController {
    client: EstateClient,
}

impl CapabilityController {
    #[must_use]
    pub fn new(client: EstateClient) -> Self {
        Self { client }
    }

    async fn reconcile_capability(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(object) = self.client.get(Kind::Capability, name).await? else {
            return Ok(Action::Done);
        };
        if is_terminating(&object) {
            return Ok(Action::Done); // teardown is the GC's job
        }
        let ResourceObject::Capability(capability) = object else {
            return Ok(Action::Done);
        };
        let desired = CapabilityStatus {
            phase: Phase::Ready,
            issued: capability.status.as_ref().map_or(0, |s| s.issued),
        };
        if capability.status.as_ref() != Some(&desired) {
            let mut converged = capability;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::Capability(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl Reconciler for CapabilityController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Capability, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_capability(name).await
        }
    }
}
