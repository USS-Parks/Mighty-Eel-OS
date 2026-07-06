//! `aogd` — the minimal Loom control-plane node daemon (Phase V, VH2).
//!
//! VH1 gave the control plane an over-the-wire Raft transport (`aog-wire`); VH2
//! packages it as a runnable **daemon**: a [`RaftNode`] on that transport, serving
//! its peer `/raft/*` endpoints alongside a thin **admin API** the conformance
//! harness drives — `initialize` / `add-learner` / `change-membership` (membership
//! carrying real peer URLs), `write` / `get`, `leader`, and `healthz`. Several of
//! these over the wire are the containerized multi-node estate the Phase-V
//! partition / kill / scale gates (V4/V5/V7/V8/V10) run on.
//!
//! Authentication, envelope sealing, and the full `aog-apiserver` CRUD are the
//! trust surface; they arrive with per-node certs + OpenBao at VH5. VH2 proves the
//! daemon forms and drives a cluster over real sockets first.

pub mod admin;
pub mod api;
pub mod client;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use aog_store::raft::RaftNode;
use aog_store::raft::types::NodeId;
use aog_wire::WireNetwork;
use axum::Router;

pub use aog_store::raft::types::RaftResponse;
pub use aog_store::{Op, Precondition, Versioned};
pub use api::{ChangeMembershipRequest, GetRequest, InitializeRequest, LeaderStatus, Member};
pub use client::{Client, ClientError};

/// A failure starting or configuring the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("config: {0}")]
    Config(String),
    #[error("node: {0}")]
    Node(#[from] aog_store::raft::NodeError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Daemon configuration — identity, storage, and where it listens / is reached.
#[derive(Debug, Clone)]
pub struct Config {
    /// This node's control-plane id.
    pub node_id: NodeId,
    /// Directory for the redb Raft log + state machine.
    pub data_dir: PathBuf,
    /// The socket the combined (raft + admin) server binds.
    pub listen: SocketAddr,
    /// The base URL peers and the harness use to reach this node — the address
    /// carried in cluster membership (defaults to `http://<listen>`).
    pub advertise: String,
}

impl Config {
    /// Read the configuration from the environment: `AOGD_NODE_ID`,
    /// `AOGD_DATA_DIR`, `AOGD_LISTEN` (a `SocketAddr`), and optional
    /// `AOGD_ADVERTISE` (defaults to `http://<listen>`).
    ///
    /// # Errors
    /// [`DaemonError::Config`] if a required variable is absent or unparseable.
    pub fn from_env() -> Result<Self, DaemonError> {
        fn required(key: &str) -> Result<String, DaemonError> {
            std::env::var(key).map_err(|_| DaemonError::Config(format!("{key} is required")))
        }
        let node_id = required("AOGD_NODE_ID")?
            .parse::<NodeId>()
            .map_err(|e| DaemonError::Config(format!("AOGD_NODE_ID: {e}")))?;
        let data_dir = PathBuf::from(required("AOGD_DATA_DIR")?);
        let listen = required("AOGD_LISTEN")?
            .parse::<SocketAddr>()
            .map_err(|e| DaemonError::Config(format!("AOGD_LISTEN: {e}")))?;
        let advertise =
            std::env::var("AOGD_ADVERTISE").unwrap_or_else(|_| format!("http://{listen}"));
        Ok(Self {
            node_id,
            data_dir,
            listen,
            advertise,
        })
    }
}

/// A running control-plane node daemon: a [`RaftNode`] on the `aog-wire` transport
/// plus the admin API, served as one axum app.
pub struct Daemon {
    node: Arc<RaftNode>,
    advertise: String,
}

impl Daemon {
    /// Start the node on the wire transport (recovering any persisted state). Does
    /// not form a cluster — the harness drives membership through the admin API.
    ///
    /// # Errors
    /// [`DaemonError::Node`] on storage or raft construction failure.
    pub async fn start(config: Config) -> Result<Self, DaemonError> {
        let node =
            RaftNode::start_with_network(config.node_id, &config.data_dir, WireNetwork::new())
                .await?;
        Ok(Self {
            node: Arc::new(node),
            advertise: config.advertise,
        })
    }

    /// The combined axum app: the `aog-wire` Raft peer endpoints (`/raft/*`) merged
    /// with the admin API (`/admin/*`, `/healthz`).
    pub fn app(&self) -> Router {
        aog_wire::router(Arc::clone(&self.node)).merge(admin::router(Arc::clone(&self.node)))
    }

    /// This daemon's Raft node handle.
    #[must_use]
    pub fn node(&self) -> Arc<RaftNode> {
        Arc::clone(&self.node)
    }

    /// The base URL peers use to reach this node.
    #[must_use]
    pub fn advertise(&self) -> &str {
        &self.advertise
    }
}
