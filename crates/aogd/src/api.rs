//! Wire DTOs shared by the admin server ([`crate::admin`]) and the harness
//! [`Client`](crate::client).

use aog_store::raft::types::NodeId;
use serde::{Deserialize, Serialize};

/// A cluster member: its id and the base URL peers reach it at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub id: NodeId,
    pub addr: String,
}

/// Body of `POST /admin/initialize` — the initial member set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub members: Vec<Member>,
}

/// Body of `POST /admin/change-membership` — the new voter set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeMembershipRequest {
    pub voters: Vec<NodeId>,
}

/// Body of `POST /admin/get` — the key to read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRequest {
    pub key: String,
}

/// Response of `GET /admin/leader` — this node's id and its leader view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderStatus {
    pub id: NodeId,
    pub leader: Option<NodeId>,
    pub is_leader: bool,
}
