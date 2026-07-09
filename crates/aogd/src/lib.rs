//! `aogd` — the minimal Loom control-plane node daemon (Phase V).
//!
//! The control plane's over-the-wire Raft transport lives in `aog-wire`; this
//! crate packages it as a runnable **daemon**: a [`RaftNode`] on that transport, serving
//! its peer `/raft/*` endpoints alongside a thin **admin API** the conformance
//! harness drives — `initialize` / `add-learner` / `change-membership` (membership
//! carrying real peer URLs), `write` / `get`, `leader`, and `healthz`. Several of
//! these over the wire are the containerized multi-node estate the Phase-V
//! partition / kill / scale gates (V4/V5/V7/V8/V10) run on.
//!
//! A trust surface layers on top. When trust material is provisioned, the daemon
//! also serves the **authenticated** `aog-apiserver` CRUD over its own node via
//! [`aog_apiserver::AppState::from_raft`] — every `/apis/**` request must carry a
//! valid trust token, fail-closed. The anchor arrives one of two ways: a raw
//! env public key (`AOGD_ANCHOR_PUBKEY`), or — taking precedence —
//! OpenBao-custodied trust material read at startup (`AOGD_OPENBAO_*`; see
//! [`provision`]), which also custodies the field-seal data key + child-mint signer
//! so sealed state is stable and shared across the estate. Per-node wire mTLS lives
//! in [`aog_wire::tls`]. The wire + admin surface still runs when no
//! anchor is set.

pub mod admin;
pub mod api;
pub mod client;
pub mod provision;

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
    /// `aog-apiserver` CRUD surface; when `None`, only the wire + admin
    /// surface is served. Ignored when `openbao` is set (that takes precedence).
    pub anchor_pubkey: Option<Vec<u8>>,
    /// OpenBao coordinates for reading the daemon's trust material at startup
    /// When set, the anchor **and** the field-seal key + signer come from
    /// the KV-v2 record at `trust_path`, taking precedence over `anchor_pubkey`.
    pub openbao: Option<OpenBaoTrust>,
}

/// OpenBao coordinates for provisioning a node's trust material. The
/// daemon logs in with the AppRole credential and reads one KV-v2 record — the
/// WSF anchor plus the field-seal data key and child-mint signer.
#[derive(Debug, Clone)]
pub struct OpenBaoTrust {
    /// OpenBao address, e.g. `http://openbao:8200`.
    pub address: String,
    /// AppRole role_id for this node.
    pub role_id: String,
    /// AppRole secret_id (pre-provisioned).
    pub secret_id: String,
    /// KV-v2 API path of the trust record, e.g. `kv/data/loom/trust`.
    pub trust_path: String,
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
        // Optional trust anchor: hex-encoded ML-DSA-87 public key.
        let anchor_pubkey = match std::env::var("AOGD_ANCHOR_PUBKEY") {
            Ok(hex_str) => Some(
                hex::decode(hex_str.trim())
                    .map_err(|e| DaemonError::Config(format!("AOGD_ANCHOR_PUBKEY: {e}")))?,
            ),
            Err(_) => None,
        };
        // Optional OpenBao trust source. When the address is set the
        // AppRole credential is required; the trust path defaults to the estate
        // convention. Takes precedence over AOGD_ANCHOR_PUBKEY at start.
        let openbao = match std::env::var("AOGD_OPENBAO_ADDR") {
            Ok(address) => Some(OpenBaoTrust {
                address,
                role_id: required("AOGD_OPENBAO_ROLE_ID")?,
                secret_id: required("AOGD_OPENBAO_SECRET_ID")?,
                trust_path: std::env::var("AOGD_OPENBAO_TRUST_PATH")
                    .unwrap_or_else(|_| "kv/data/loom/trust".to_owned()),
            }),
            Err(_) => None,
        };
        Ok(Self {
            node_id,
            data_dir,
            listen,
            advertise,
            anchor_pubkey,
            openbao,
        })
    }
}

/// A running control-plane node daemon: a [`RaftNode`] on the `aog-wire` transport
/// plus the admin API, served as one axum app.
pub struct Daemon {
    node: Arc<RaftNode>,
    advertise: String,
    /// Authenticated API state, present when an anchor was provisioned.
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
        // When trust material is provisioned, serve the authenticated
        // aog-apiserver CRUD over this very node (the `from_raft` seam), fail-closed.
        // Precedence: OpenBao-custodied material over the env anchor +
        // ephemeral kernel sealer. Absent both, only the wire + admin
        // surface runs.
        let state = if let Some(bao) = &config.openbao {
            let material = provision::from_openbao(bao).await?;
            Some(AppState::from_raft(
                Arc::clone(&node),
                material.authenticator,
                material.sealer,
            ))
        } else if let Some(pubkey) = config.anchor_pubkey {
            let authenticator = Authenticator::new(pubkey);
            let sealer =
                Sealer::generate().map_err(|e| DaemonError::Config(format!("sealer: {e}")))?;
            Some(AppState::from_raft(
                Arc::clone(&node),
                authenticator,
                sealer,
            ))
        } else {
            None
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
        let mut app = aog_wire::router(Arc::clone(&self.node)).merge(admin::router(
            Arc::clone(&self.node),
            self.state
                .as_ref()
                .map(aog_apiserver::AppState::authenticator),
        ));
        // The authenticated CRUD surface, when an anchor is provisioned.
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
