//! The in-process multi-node transport. A [`Cluster`] is a registry of the
//! peer `Raft` handles; the [`ClusterNetwork`] factory routes openraft's
//! `append_entries` / `vote` / `install_snapshot` RPCs by direct call to the
//! target's handle. This runs **real** openraft consensus — election,
//! replication, commit — across ≥3 nodes in one process, which is what proves
//! leader failover and no-lost-committed-state without a network.
//!
//! An `isolated` set injects a partition: a node in it neither sends nor receives
//! RPCs, exactly as a severed link would. It is used to fence a minority and
//! assert split-brain safety; here it is the seam. (A real over-the-wire mTLS
//! transport is the deployment packaging; the consensus correctness it must carry
//! is what this harness pins.)

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex};

use openraft::BasicNode;
use openraft::Raft;
use openraft::error::{RPCError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};

use crate::raft::types::{NodeId, TypeConfig};

/// An in-process cluster: the registered peer Raft handles and the current
/// partition. Cheap to clone (all state is shared behind `Arc`).
#[derive(Clone, Default)]
pub struct Cluster {
    peers: Arc<Mutex<HashMap<NodeId, Raft<TypeConfig>>>>,
    isolated: Arc<Mutex<HashSet<NodeId>>>,
}

impl Cluster {
    /// An empty cluster — register nodes as they start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node's Raft handle so peers can reach it.
    pub fn register(&self, id: NodeId, raft: Raft<TypeConfig>) {
        self.peers
            .lock()
            .expect("cluster poisoned")
            .insert(id, raft);
    }

    /// Partition `id` off the network (it can neither send nor receive RPCs).
    pub fn isolate(&self, id: NodeId) {
        self.isolated.lock().expect("cluster poisoned").insert(id);
    }

    /// Heal `id` back onto the network.
    pub fn heal(&self, id: NodeId) {
        self.isolated.lock().expect("cluster poisoned").remove(&id);
    }

    /// Heal every partition.
    pub fn heal_all(&self) {
        self.isolated.lock().expect("cluster poisoned").clear();
    }

    fn reachable(&self, from: NodeId, to: NodeId) -> bool {
        let isolated = self.isolated.lock().expect("cluster poisoned");
        !isolated.contains(&from) && !isolated.contains(&to)
    }

    fn peer(&self, id: NodeId) -> Option<Raft<TypeConfig>> {
        self.peers
            .lock()
            .expect("cluster poisoned")
            .get(&id)
            .cloned()
    }

    /// The network factory for the node with id `from` (so partition checks know
    /// which side an RPC originates on).
    #[must_use]
    pub fn network(&self, from: NodeId) -> ClusterNetwork {
        ClusterNetwork {
            cluster: self.clone(),
            from,
        }
    }
}

/// A [`RaftNetworkFactory`] bound to one origin node.
#[derive(Clone)]
pub struct ClusterNetwork {
    cluster: Cluster,
    from: NodeId,
}

/// A connection from `from` to one `to` peer.
pub struct ClusterConnection {
    cluster: Cluster,
    from: NodeId,
    to: NodeId,
}

impl RaftNetworkFactory<TypeConfig> for ClusterNetwork {
    type Network = ClusterConnection;

    async fn new_client(&mut self, target: NodeId, _node: &BasicNode) -> Self::Network {
        ClusterConnection {
            cluster: self.cluster.clone(),
            from: self.from,
            to: target,
        }
    }
}

fn severed(from: NodeId, to: NodeId) -> Unreachable {
    Unreachable::new(&io::Error::other(format!(
        "link {from}->{to} is partitioned"
    )))
}

fn absent(to: NodeId) -> Unreachable {
    Unreachable::new(&io::Error::other(format!("peer {to} is not registered")))
}

impl ClusterConnection {
    /// The reachable target handle, or the small `Unreachable` reason (the caller
    /// lifts it into the large `RPCError` at the trait boundary).
    fn locate(&self) -> Result<Raft<TypeConfig>, Unreachable> {
        if !self.cluster.reachable(self.from, self.to) {
            return Err(severed(self.from, self.to));
        }
        self.cluster.peer(self.to).ok_or_else(|| absent(self.to))
    }
}

impl RaftNetwork<TypeConfig> for ClusterConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        AppendEntriesResponse<NodeId>,
        RPCError<NodeId, BasicNode, openraft::error::RaftError<NodeId>>,
    > {
        let peer = self.locate().map_err(RPCError::Unreachable)?;
        peer.append_entries(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.to, e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<
            NodeId,
            BasicNode,
            openraft::error::RaftError<NodeId, openraft::error::InstallSnapshotError>,
        >,
    > {
        let peer = self.locate().map_err(RPCError::Unreachable)?;
        peer.install_snapshot(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.to, e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, openraft::error::RaftError<NodeId>>>
    {
        let peer = self.locate().map_err(RPCError::Unreachable)?;
        peer.vote(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.to, e)))
    }
}
