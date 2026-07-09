//! The TrustRing controller: rings become per-ring OpenBao Transit keys,
//! and a ring can be **darkened** — its key disabled so every envelope sealed
//! under it stops unsealing, and its workloads are halted.
//!
//! Reconciles two kinds through one reconciler (run it on a `"TrustRing/"`
//! informer and a `"RevocationIntent/"` informer):
//!
//! * a live `TrustRing` gets its Transit key ensured (`loom-ring-<n>`) and a
//!   `Ready` status carrying the key version;
//! * a `RevocationIntent` targeting `Ring(n)` darkens every `TrustRing`
//!   declaring ring `n`: the Transit key is disabled (envelopes under it are
//!   unreadable from that moment — the data-key wraps can no longer be
//!   decrypted), every `Workload` in the ring is marked `Failed` (the estate
//!   halt; the M3b node runtime enforces eviction), the ring's status goes
//!   `dark`/`Degraded`, and the intent is acknowledged `Ready`/propagated —
//!   the acknowledgment the indexer honestly refused to fake.
//!
//! Deleting a `TrustRing` **retains** its Transit key (reclaim-policy Retain):
//! sealed data must never become unreadable because an estate object was
//! dropped — darkness is an explicit, receipted `RevocationIntent`, not a
//! side effect.

use std::future::Future;
use std::sync::Arc;

use aog_estate::{Kind, Phase, ResourceObject, RevocationTarget, TrustRingStatus};

use crate::objects::{EstateClient, is_terminating, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};
use crate::transit::{TransitAdmin, ring_key_name};

/// TrustRing lifecycle controller.
#[derive(Clone)]
pub struct TrustRingController {
    client: EstateClient,
    transit: Arc<TransitAdmin>,
}

impl TrustRingController {
    #[must_use]
    pub fn new(client: EstateClient, transit: Arc<TransitAdmin>) -> Self {
        Self { client, transit }
    }

    /// Is there a (non-terminating) revocation intent darkening `ring`?
    async fn ring_darkened(&self, ring: u8) -> Result<bool, ReconcileError> {
        for object in self.client.list(Kind::RevocationIntent).await? {
            let ResourceObject::RevocationIntent(intent) = &object else {
                continue;
            };
            if intent.spec.target == RevocationTarget::Ring(ring) && !is_terminating(&object) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Halt every workload declared in `ring` (estate leg: status `Failed`).
    async fn halt_ring_workloads(&self, ring: u8) -> Result<(), ReconcileError> {
        for object in self.client.list(Kind::Workload).await? {
            let ResourceObject::Workload(workload) = object else {
                continue;
            };
            if workload.spec.ring != ring {
                continue;
            }
            let halted = workload
                .status
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Failed && s.ready_replicas == 0);
            if halted {
                continue;
            }
            let mut halt = workload;
            let status = halt.status.get_or_insert_with(Default::default);
            status.phase = Phase::Failed;
            status.ready_replicas = 0;
            self.client.update(ResourceObject::Workload(halt)).await?;
        }
        Ok(())
    }

    /// Acknowledge every intent darkening `ring` as enforced.
    async fn ack_ring_intents(&self, ring: u8) -> Result<(), ReconcileError> {
        for object in self.client.list(Kind::RevocationIntent).await? {
            let ResourceObject::RevocationIntent(intent) = object else {
                continue;
            };
            if intent.spec.target != RevocationTarget::Ring(ring)
                || intent.status.as_ref().is_some_and(|s| s.propagated)
            {
                continue;
            }
            let mut acked = intent;
            let status = acked.status.get_or_insert_with(Default::default);
            status.phase = Phase::Ready;
            status.propagated = true;
            status.replicas_denied = 1; // this replica; the revocation controller counts the estate
            self.client
                .update(ResourceObject::RevocationIntent(acked))
                .await?;
        }
        Ok(())
    }

    async fn reconcile_ring(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::TrustRing(ring)) = self.client.get(Kind::TrustRing, name).await?
        else {
            return Ok(Action::Done);
        };
        // Deletion retains the key: darkness is declared, never a side effect.
        if ring.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done);
        }

        let key = ring_key_name(ring.spec.ring);
        let desired = if self.ring_darkened(ring.spec.ring).await? {
            // Dark: disable the key family — the base ring key AND every
            // per-tenant derivative (E2 namespacing) — halt the ring's
            // workloads, ack the kill. A tenant-scoped wrap must not survive.
            self.transit.disable_key_family(&key).await?;
            self.halt_ring_workloads(ring.spec.ring).await?;
            self.ack_ring_intents(ring.spec.ring).await?;
            TrustRingStatus {
                phase: Phase::Degraded,
                key_version: ring.status.as_ref().and_then(|s| s.key_version),
                dark: true,
            }
        } else {
            // Live: the ring's Transit key exists and is reported.
            let version = self.transit.ensure_key(&key).await?;
            TrustRingStatus {
                phase: Phase::Ready,
                key_version: Some(version),
                dark: false,
            }
        };

        if ring.status.as_ref() != Some(&desired) {
            let mut converged = ring;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::TrustRing(converged))
                .await?;
        }
        Ok(Action::Done)
    }

    /// A ring-target intent wakes every `TrustRing` declaring that ring.
    async fn reconcile_intent(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::RevocationIntent(intent)) =
            self.client.get(Kind::RevocationIntent, name).await?
        else {
            return Ok(Action::Done);
        };
        let RevocationTarget::Ring(ring) = intent.spec.target else {
            return Ok(Action::Done); // token/subject/tenant intents are the indexer's
        };
        for object in self.client.list(Kind::TrustRing).await? {
            let ResourceObject::TrustRing(r) = &object else {
                continue;
            };
            if r.spec.ring == ring {
                self.reconcile_ring(object.name()).await?;
            }
        }
        Ok(Action::Done)
    }
}

impl Reconciler for TrustRingController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            match parse_key(&key) {
                Some((Kind::TrustRing, name)) => controller.reconcile_ring(name).await,
                Some((Kind::RevocationIntent, name)) => controller.reconcile_intent(name).await,
                _ => Ok(Action::Done),
            }
        }
    }
}
