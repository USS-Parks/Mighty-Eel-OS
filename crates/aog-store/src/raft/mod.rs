//! The Raft node over the store. openraft (0.9) on top of redb:
//! linearizable writes via `client_write`, committed state durable across a
//! restart. A single voter runs with a no-peer stub network; the multi-node path
//! adds [`RaftNode::join`] onto a [`Cluster`], `add_learner` + `change_membership`
//! to form a ≥3-node control plane, leader election/failover, and a leadership
//! [`RaftNode::leadership`] watch that drives a controller's `SharedGate` so only
//! the leader reconciles.

pub mod cluster;
pub mod log_store;
pub mod network;
pub mod state_machine;
pub mod types;
pub mod watch;

pub use cluster::Cluster;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openraft::network::RaftNetworkFactory;
use openraft::{BasicNode, Config, Raft, ServerState};
use redb::Database;

use crate::raft::log_store::RedbLogStore;
use crate::raft::network::SingleNodeNetwork;
use crate::raft::state_machine::RedbStateMachine;
use crate::raft::types::{NodeId, RaftRequest, RaftResponse, TypeConfig};
use crate::{Op, Revision, Versioned};

/// A failure operating the Raft node. openraft's rich error types are
/// stringified at this boundary; callers act on the variant, not the detail.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("raft: {0}")]
    Raft(String),
    #[error("io: {0}")]
    Io(String),
}

/// A running single-node Loom control-plane Raft node.
pub struct RaftNode {
    raft: Raft<TypeConfig>,
    sm: RedbStateMachine,
    node_id: NodeId,
}

impl RaftNode {
    /// Start the node over `dir` (creating the redb stores), recovering any
    /// persisted log + state machine, with the no-peer network. Does **not**
    /// initialize a cluster — use [`RaftNode::bootstrap`] for a fresh single-node
    /// estate or [`RaftNode::join`] for a multi-node one.
    ///
    /// # Errors
    /// [`NodeError`] on storage or raft construction failure.
    pub async fn start(node_id: NodeId, dir: impl AsRef<Path>) -> Result<Self, NodeError> {
        Self::build(node_id, dir, SingleNodeNetwork).await
    }

    /// Start the node over `dir` wired to a caller-supplied `network` factory —
    /// the seam for the over-the-wire mTLS transport (the containerized multi-node
    /// harness). Does not initialize a cluster; drive membership via
    /// [`raft`](Self::raft) with `BasicNode` addresses carrying peer URLs.
    ///
    /// # Errors
    /// [`NodeError`] on storage or raft construction failure.
    pub async fn start_with_network<N>(
        node_id: NodeId,
        dir: impl AsRef<Path>,
        network: N,
    ) -> Result<Self, NodeError>
    where
        N: RaftNetworkFactory<TypeConfig>,
    {
        Self::build(node_id, dir, network).await
    }

    /// Start the node over `dir` wired to `network` — the no-peer stub, a
    /// [`Cluster`] network.
    async fn build<N>(node_id: NodeId, dir: impl AsRef<Path>, network: N) -> Result<Self, NodeError>
    where
        N: RaftNetworkFactory<TypeConfig>,
    {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| NodeError::Io(e.to_string()))?;

        let log_db = Arc::new(
            Database::create(dir.join("raft-log.redb"))
                .map_err(|e| NodeError::Storage(e.to_string()))?,
        );
        let log_store =
            RedbLogStore::open(log_db).map_err(|e| NodeError::Storage(e.to_string()))?;
        let sm = RedbStateMachine::open(dir).map_err(|e| NodeError::Storage(e.to_string()))?;

        let config = Config {
            cluster_name: "loom".to_owned(),
            ..Config::default()
        };
        let config = Arc::new(
            config
                .validate()
                .map_err(|e| NodeError::Raft(e.to_string()))?,
        );

        let raft = Raft::new(node_id, config, network, log_store, sm.clone())
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;

        Ok(Self { raft, sm, node_id })
    }

    /// Start a node wired to `cluster` and register its handle so peers can reach
    /// it. It joins un-initialized; the bootstrap node forms the cluster with
    /// [`initialize`](Self::initialize) then [`add_learner`](Self::add_learner) +
    /// [`change_membership`](Self::change_membership).
    ///
    /// # Errors
    /// [`NodeError`] on storage or raft construction failure.
    pub async fn join(
        node_id: NodeId,
        dir: impl AsRef<Path>,
        cluster: &Cluster,
    ) -> Result<Self, NodeError> {
        let node = Self::build(node_id, dir, cluster.network(node_id)).await?;
        cluster.register(node_id, node.raft.clone());
        Ok(node)
    }

    /// Start the node and initialize a fresh single-voter cluster, waiting until
    /// it is leader (so writes commit immediately). Idempotent: re-bootstrapping
    /// an already-initialized estate just re-establishes leadership.
    ///
    /// # Errors
    /// [`NodeError`] on storage/raft failure, or if leadership is not reached.
    pub async fn bootstrap(node_id: NodeId, dir: impl AsRef<Path>) -> Result<Self, NodeError> {
        let node = Self::start(node_id, dir).await?;

        let mut members = BTreeMap::new();
        members.insert(node_id, BasicNode::new(""));
        // Ignore AlreadyInitialized on a restart-into-bootstrap.
        let _ = node.raft.initialize(members).await;

        node.raft
            .wait(Some(Duration::from_secs(10)))
            .state(ServerState::Leader, "single-node estate becomes leader")
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;

        Ok(node)
    }

    /// Initialize a fresh cluster with `voters` as its initial members and wait
    /// for a leader. Call once, on the bootstrap node.
    ///
    /// # Errors
    /// [`NodeError::Raft`] on a raft failure.
    pub async fn initialize(&self, voters: BTreeSet<NodeId>) -> Result<(), NodeError> {
        let members: BTreeMap<NodeId, BasicNode> = voters
            .into_iter()
            .map(|id| (id, BasicNode::new("")))
            .collect();
        self.raft
            .initialize(members)
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        Ok(())
    }

    /// Add `id` as a learner (non-voting) and wait for it to catch up — the first
    /// step of admitting a node to the cluster. The node must already have
    /// [`join`](Self::join)ed.
    ///
    /// # Errors
    /// [`NodeError::Raft`] on a raft failure.
    pub async fn add_learner(&self, id: NodeId) -> Result<(), NodeError> {
        self.raft
            .add_learner(id, BasicNode::new(""), true)
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        Ok(())
    }

    /// Set the cluster's voter set to `voters` (promotes caught-up learners to
    /// voters, or removes a member).
    ///
    /// # Errors
    /// [`NodeError::Raft`] on a raft failure.
    pub async fn change_membership(&self, voters: BTreeSet<NodeId>) -> Result<(), NodeError> {
        self.raft
            .change_membership(voters, false)
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        Ok(())
    }

    /// Whether this node is the current Raft leader — the signal a controller's
    /// `SharedGate` follows so only the leader reconciles.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        matches!(self.raft.metrics().borrow().state, ServerState::Leader)
    }

    /// The cluster's current leader, if one is established.
    #[must_use]
    pub fn current_leader(&self) -> Option<NodeId> {
        self.raft.metrics().borrow().current_leader
    }

    /// Wait up to `timeout` for a leader to be established, returning its id — how
    /// a failover test asserts a new leader emerged within SLO.
    ///
    /// # Errors
    /// [`NodeError::Raft`] if no leader emerges within `timeout`.
    pub async fn wait_for_leader(&self, timeout: Duration) -> Result<NodeId, NodeError> {
        self.raft
            .wait(Some(timeout))
            .metrics(|m| m.current_leader.is_some(), "a leader is elected")
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        self.current_leader()
            .ok_or_else(|| NodeError::Raft("leader elected but not reported".to_owned()))
    }

    /// **Quorum-confirmed** leadership (fencing): performs a ReadIndex
    /// (openraft `ensure_linearizable`) that only returns `Ok` when a quorum still
    /// acknowledges this node as leader. A partitioned minority leader — which in
    /// classic Raft still *believes* it leads — cannot confirm a quorum and
    /// returns `false`, so it fences and serves no authoritative decision. This is
    /// the split-brain-safe check the trust path (not the metrics view) must use.
    pub async fn confirm_leadership(&self, timeout: Duration) -> bool {
        matches!(
            tokio::time::timeout(timeout, self.raft.ensure_linearizable()).await,
            Ok(Ok(_))
        )
    }

    /// A watch of this node's leadership, updated on every Raft state change — the
    /// wiring a `SharedGate` follows, so losing leadership stops this replica
    /// reconciling on the next pass (fail-closed for action, doctrine I-4).
    #[must_use]
    pub fn leadership(&self) -> tokio::sync::watch::Receiver<bool> {
        let mut metrics = self.raft.metrics();
        let initial = matches!(metrics.borrow().state, ServerState::Leader);
        let (tx, rx) = tokio::sync::watch::channel(initial);
        tokio::spawn(async move {
            while metrics.changed().await.is_ok() {
                let leader = matches!(metrics.borrow().state, ServerState::Leader);
                if tx.send(leader).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// This node's id.
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.node_id
    }

    /// This node's openraft handle — the surface the wire transport's server side
    /// serves (`append_entries` / `vote` / `install_snapshot`) and the daemon
    /// drives membership through (`initialize` / `add_learner` / `change_membership`
    /// with `BasicNode` peer URLs). Cheap to clone.
    #[must_use]
    pub fn raft(&self) -> Raft<TypeConfig> {
        self.raft.clone()
    }

    /// Trigger a Raft snapshot (compaction) and wait until it is built to the
    /// applied index, so the state machine is captured and the log before it can
    /// be purged. A node recovers the same estate from the snapshot + the log tail.
    ///
    /// # Errors
    /// [`NodeError::Raft`] if the snapshot cannot be triggered or is not built in
    /// `timeout`.
    pub async fn snapshot(&self, timeout: Duration) -> Result<(), NodeError> {
        let target = self.raft.metrics().borrow().last_applied;
        self.raft
            .trigger()
            .snapshot()
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        if let Some(target) = target {
            self.raft
                .wait(Some(timeout))
                .metrics(
                    move |m| m.snapshot.is_some_and(|s| s.index >= target.index),
                    "snapshot built to the applied index",
                )
                .await
                .map_err(|e| NodeError::Raft(e.to_string()))?;
        }
        Ok(())
    }

    /// The log index of the last snapshot this node has taken, if any.
    #[must_use]
    pub fn last_snapshot(&self) -> Option<u64> {
        self.raft.metrics().borrow().snapshot.map(|s| s.index)
    }

    /// Range the committed state by key prefix.
    ///
    /// # Errors
    /// [`NodeError::Storage`] on backend failure.
    pub async fn range(&self, prefix: &str) -> Result<Vec<(String, Versioned)>, NodeError> {
        self.sm
            .range(prefix)
            .await
            .map_err(|e| NodeError::Storage(e.to_string()))
    }

    /// A prefix-scoped [`Informer`](crate::raft::watch::Informer) over this
    /// node's committed state (read path for controllers).
    #[must_use]
    pub fn informer(&self, prefix: impl Into<String>) -> crate::raft::watch::Informer {
        crate::raft::watch::Informer::new(self.sm.clone(), prefix)
    }

    /// Linearizably replicate and apply one desired-state mutation. A failed
    /// precondition returns [`RaftResponse::Rejected`], not an error.
    ///
    /// # Errors
    /// [`NodeError::Raft`] if the write cannot be committed (e.g. not leader).
    pub async fn write(&self, op: Op) -> Result<RaftResponse, NodeError> {
        let response = self
            .raft
            .client_write(RaftRequest::from(op))
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;
        Ok(response.data)
    }

    /// Read one applied key from the committed state machine.
    ///
    /// # Errors
    /// [`NodeError::Storage`] on backend failure.
    pub async fn get(&self, key: &str) -> Result<Option<Versioned>, NodeError> {
        self.sm
            .get(key)
            .await
            .map_err(|e| NodeError::Storage(e.to_string()))
    }

    /// The applied global revision.
    pub async fn revision(&self) -> Revision {
        self.sm.revision().await
    }

    /// Gracefully stop the Raft core.
    ///
    /// # Errors
    /// [`NodeError::Raft`] if shutdown fails.
    pub async fn shutdown(self) -> Result<(), NodeError> {
        self.raft
            .shutdown()
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))
    }
}
