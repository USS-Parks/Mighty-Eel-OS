//! openraft (0.9) type configuration for `aog-store`.

use serde::{Deserialize, Serialize};

use crate::{Op, Revision};

/// Control-plane node id. Single-node now; multi-node later.
pub type NodeId = u64;

/// One replicated desired-state mutation — openraft's app data `D`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftRequest {
    pub op: Op,
}

impl From<Op> for RaftRequest {
    fn from(op: Op) -> Self {
        Self { op }
    }
}

/// The state machine's reply to an applied request — openraft's `R`. An
/// application-level rejection (a failed CAS) is a value here, never a Raft
/// error: consensus succeeds, the write is refused (fail-closed at the store).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RaftResponse {
    Applied { revision: Revision, created: bool },
    Deleted { revision: Revision },
    Rejected { reason: String },
    Noop,
}

openraft::declare_raft_types!(
    /// The Loom control-plane Raft types.
    pub TypeConfig:
        D = RaftRequest,
        R = RaftResponse,
        NodeId = NodeId,
        Node = openraft::BasicNode,
        Entry = openraft::Entry<TypeConfig>,
        SnapshotData = std::io::Cursor<Vec<u8>>,
        AsyncRuntime = openraft::TokioRuntime,
);
