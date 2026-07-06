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
//! VH5b lands the trust surface's first leg: when an anchor public key is
//! provisioned (`AOGD_ANCHOR_PUBKEY`), the daemon also serves the **authenticated**
//! `aog-apiserver` CRUD over its own node via [`aog_apiserver::AppState::from_raft`]
//! — every `/apis/**` request must carry a valid trust token (K6), fail-closed.
//! Per-node mTLS on the wire and OpenBao-provisioned anchors are the remaining
//! VH5b legs; the VH2 wire + admin surface still runs when no anchor is set.

pub mod admin;
pub mod api;
pub mod client;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
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
    /// The WSF trust-anchor public key (raw ML-DSA-87 bytes) every presented token
    /// must verify under. When set, the daemon serves the **authenticated**
    /// `aog-apiserver` CRUD surface (VH5b); when `None`, only the VH2 wire + admin
    /// surface is served.
    pub anchor_pubkey: Option<Vec<u8>>,
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
        // Optional VH5b trust anchor: hex-encoded ML-DSA-87 public key.
        let anchor_pubkey = match std::env::var("AOGD_ANCHOR_PUBKEY") {
            Ok(hex_str) => Some(
                hex::decode(hex_str.trim())
                    .map_err(|e| DaemonError::Config(format!("AOGD_ANCHOR_PUBKEY: {e}")))?,
            ),
            Err(_) => None,
        };
        Ok(Self {
            node_id,
            data_dir,
            listen,
            advertise,
            anchor_pubkey,
        })
    }
}

/// A running control-plane node daemon: a [`RaftNode`] on the `aog-wire` transport
/// plus the admin API, served as one axum app.
pub struct Daemon {
    node: Arc<RaftNode>,
    advertise: String,
    /// Authenticated API state (VH5b), present when an anchor was provisioned.
    state: Option<AppState>,
}

impl Daemon {
    /// Start the node on the wire transport (recovering any persisted state). Does
    /// not form a cluster — the harness drives membership through the admin API.
    ///
    /// # Errors
    /// [`DaemonError::Node`] on storage or raft construction failure.
    pub async fn start(config: Config) -> Result<Self, DaemonError> {
        let node = Arc::new(
            RaftNode::start_with_network(config.node_id, &config.data_dir, WireNetwork::new())
                .await?,
        );
        // VH5b: when an anchor public key is provisioned, serve the authenticated
        // aog-apiserver CRUD over this very node (the `from_raft` seam). Fail closed
        // on a bad sealer. Absent an anchor, only the VH2 wire + admin surface runs.
        let state = match config.anchor_pubkey {
            Some(pubkey) => {
                let authenticator = Authenticator::new(pubkey);
                let sealer =
                    Sealer::generate().map_err(|e| DaemonError::Config(format!("sealer: {e}")))?;
                Some(AppState::from_raft(
                    Arc::clone(&node),
                    authenticator,
                    sealer,
                ))
            }
            None => None,
        };
        Ok(Self {
            node,
            advertise: config.advertise,
            state,
        })
    }

    /// The combined axum app: the `aog-wire` Raft peer endpoints (`/raft/*`) merged
    /// with the admin API (`/admin/*`, `/healthz`).
    pub fn app(&self) -> Router {
        let mut app =
            aog_wire::router(Arc::clone(&self.node)).merge(admin::router(Arc::clone(&self.node)));
        // VH5b: the authenticated CRUD surface, when an anchor is provisioned.
        if let Some(state) = &self.state {
            app = app.merge(aog_apiserver::api_router(state.clone()));
        }
        app
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
