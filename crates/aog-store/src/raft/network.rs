//! Single-node network. There are no peers, so these RPCs are never
//! issued; openraft still requires a `RaftNetworkFactory` for its type surface.
//! A real transport replaces this (mTLS, sender-constrained per I-3).

use std::io;

use openraft::BasicNode;
use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};

use crate::raft::types::{NodeId, TypeConfig};

/// Factory that hands out no-op peer connections.
#[derive(Debug, Clone, Default)]
pub struct SingleNodeNetwork;

/// A connection to a peer that does not exist in a single-node estate.
#[derive(Debug, Clone, Default)]
pub struct SingleNodeConnection;

fn no_peers() -> Unreachable {
    Unreachable::new(&io::Error::other("single-node estate has no peers"))
}

impl RaftNetworkFactory<TypeConfig> for SingleNodeNetwork {
    type Network = SingleNodeConnection;

    async fn new_client(&mut self, _target: NodeId, _node: &BasicNode) -> Self::Network {
        SingleNodeConnection
    }
}

impl RaftNetwork<TypeConfig> for SingleNodeConnection {
    async fn append_entries(
        &mut self,
        _rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        Err(RPCError::Unreachable(no_peers()))
    }

    async fn install_snapshot(
        &mut self,
        _rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        Err(RPCError::Unreachable(no_peers()))
    }

    async fn vote(
        &mut self,
        _rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        Err(RPCError::Unreachable(no_peers()))
    }
}
