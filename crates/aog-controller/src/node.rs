//! The node controller enforces liveness. A node whose
//! heartbeat has aged past the freshness window, or that reports not-ready, is
//! marked down and its `Placement`s are evicted, so the scheduler re-places
//! those replicas on live nodes — the "killed node reschedules" guarantee.
//! Fail-closed: a silent node loses candidacy (I-4). Staleness is re-checked on
//! each resync, since a silent node emits no event of its own.

use std::future::Future;

use chrono::{DateTime, Utc};

use aog_estate::{Kind, NodeStatus, Phase, ResourceObject};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// Enforces node liveness and reschedules a dead node's workloads.
#[derive(Clone)]
pub struct NodeController {
    client: EstateClient,
    heartbeat_ttl_secs: i64,
}

impl NodeController {
    /// A node is not-live once its heartbeat is older than `heartbeat_ttl_secs`
    /// (or it reports not-ready).
    #[must_use]
    pub fn new(client: EstateClient, heartbeat_ttl_secs: i64) -> Self {
        Self {
            client,
            heartbeat_ttl_secs,
        }
    }

    async fn reconcile_node(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Node(node)) = self.client.get(Kind::Node, name).await? else {
            return Ok(Action::Done);
        };
        if node.metadata.deletion_timestamp.is_some() {
            self.evict_placements(name).await?; // a removed node keeps no placements
            return Ok(Action::Done);
        }

        let ready = node.status.as_ref().is_some_and(|s| s.ready);
        let live = ready
            && node
                .status
                .as_ref()
                .is_some_and(|s| !stale(s, Utc::now(), self.heartbeat_ttl_secs));
        if live {
            return Ok(Action::Done);
        }

        // Not live: mark it down (so the scheduler stops choosing it) and evict
        // its placements (so the scheduler re-places their replicas elsewhere).
        if ready {
            let mut down = node;
            let mut status = down.status.take().unwrap_or_default();
            status.ready = false;
            status.phase = Phase::Degraded;
            down.status = Some(status);
            self.client.update(ResourceObject::Node(down)).await?;
        }
        self.evict_placements(name).await?;
        Ok(Action::Done)
    }

    async fn evict_placements(&self, node: &str) -> Result<(), ReconcileError> {
        for object in self.client.list(Kind::Placement).await? {
            if let ResourceObject::Placement(placement) = object
                && placement.spec.node == node
            {
                self.client
                    .delete(Kind::Placement, &placement.metadata.name)
                    .await?;
            }
        }
        Ok(())
    }
}

/// Whether `status`'s heartbeat has aged past `ttl_secs` (fail-closed on a
/// missing or unparseable timestamp).
fn stale(status: &NodeStatus, now: DateTime<Utc>, ttl_secs: i64) -> bool {
    let Some(beat) = status.last_heartbeat.as_deref() else {
        return true;
    };
    let Ok(beat) = DateTime::parse_from_rfc3339(beat) else {
        return true;
    };
    (now - beat.with_timezone(&Utc)).num_seconds() > ttl_secs
}

impl Reconciler for NodeController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Node, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_node(name).await
        }
    }
}
