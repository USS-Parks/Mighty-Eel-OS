//! K3 — the single-node Raft node over the K2 store. openraft (0.9) on top of
//! redb: linearizable writes via `client_write`, committed state durable across
//! a restart. The estate is one voter here; multi-node election/replication is
//! wired at H1 (this module's network is a no-peer stub).

pub mod log_store;
pub mod network;
pub mod state_machine;
pub mod types;
pub mod watch;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
    /// persisted log + state machine. Does **not** initialize a cluster — use
    /// [`RaftNode::bootstrap`] for a fresh estate.
    ///
    /// # Errors
    /// [`NodeError`] on storage or raft construction failure.
    pub async fn start(node_id: NodeId, dir: impl AsRef<Path>) -> Result<Self, NodeError> {
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

        let raft = Raft::new(node_id, config, SingleNodeNetwork, log_store, sm.clone())
            .await
            .map_err(|e| NodeError::Raft(e.to_string()))?;

        Ok(Self { raft, sm, node_id })
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

    /// This node's id.
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.node_id
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
    /// node's committed state (K4 read path for controllers).
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
