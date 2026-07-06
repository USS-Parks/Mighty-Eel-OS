//! `aog-noded` — the runnable Loom edge/worker node daemon (Phase V, VH3).
//!
//! VH2 made the control plane runnable (`aogd`); VH3 makes the **edge** runnable.
//! `aog-noded` registers its `Node` (attestation profile + capacity) with the
//! control plane over the wire — through the VH2 admin API, reusing `aogd::Client`
//! — and heartbeats its liveness on an interval, so the control plane sees the edge
//! join the estate and stay `Ready` (the N1/N2 lifecycle, now over sockets). It
//! serves `/healthz` for the harness.
//!
//! Identity-verified registration (the `aog-node` `Registrar` / anchor-signed leaf)
//! and edge admission over live OpenBao are the trust surface; like VH2 they layer
//! on at VH5 (per-node certs + OpenBao). VH3 proves the edge joins and stays live
//! over real sockets first.

use std::net::SocketAddr;
use std::time::Duration;

use aog_estate::{AttestationProfile, Capacity, Node, NodeSpec, Resource};
use aog_node::heartbeat::heartbeat;
use aogd::{Client, Op, Precondition};
use axum::Router;
use axum::routing::get;
use chrono::Utc;
use fabric_contracts::Classification;

/// A failure configuring or running the node daemon.
#[derive(Debug, thiserror::Error)]
pub enum NodedError {
    #[error("config: {0}")]
    Config(String),
    #[error("control plane: {0}")]
    ControlPlane(#[from] aogd::ClientError),
    #[error("encode: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Edge node daemon configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The node's name (its `Node` resource name + identity subject).
    pub name: String,
    /// The tenant the node belongs to.
    pub tenant: String,
    /// The trust ring the node serves.
    pub ring: u8,
    /// The highest classification the node is attested to hold (the S4 floor).
    pub attestation_floor: Classification,
    /// The node's declared capacity.
    pub capacity: Capacity,
    /// Base URL of a control-plane `aogd` (e.g. `http://cp1:4600`).
    pub control_plane: String,
    /// The socket the node's `/healthz` server binds.
    pub listen: SocketAddr,
    /// How often the node re-reports its heartbeat.
    pub heartbeat: Duration,
}

impl NodeConfig {
    /// Read the configuration from the environment: `AOG_NODE_NAME`,
    /// `AOG_NODE_CONTROL_PLANE`, and `AOG_NODE_LISTEN` (a `SocketAddr`) are
    /// required; `AOG_NODE_TENANT` (default `default`), `AOG_NODE_RING` (default
    /// `1`), and `AOG_NODE_HEARTBEAT_SECS` (default `5`) are optional. Attestation
    /// floor and capacity take conservative defaults (attested for real at VH5).
    ///
    /// # Errors
    /// [`NodedError::Config`] if a required variable is absent or unparseable.
    pub fn from_env() -> Result<Self, NodedError> {
        fn required(key: &str) -> Result<String, NodedError> {
            std::env::var(key).map_err(|_| NodedError::Config(format!("{key} is required")))
        }
        fn optional(key: &str, default: &str) -> String {
            std::env::var(key).unwrap_or_else(|_| default.to_owned())
        }
        let name = required("AOG_NODE_NAME")?;
        let control_plane = required("AOG_NODE_CONTROL_PLANE")?;
        let listen = required("AOG_NODE_LISTEN")?
            .parse::<SocketAddr>()
            .map_err(|e| NodedError::Config(format!("AOG_NODE_LISTEN: {e}")))?;
        let tenant = optional("AOG_NODE_TENANT", "default");
        let ring = optional("AOG_NODE_RING", "1")
            .parse::<u8>()
            .map_err(|e| NodedError::Config(format!("AOG_NODE_RING: {e}")))?;
        let heartbeat_secs = optional("AOG_NODE_HEARTBEAT_SECS", "5")
            .parse::<u64>()
            .map_err(|e| NodedError::Config(format!("AOG_NODE_HEARTBEAT_SECS: {e}")))?;
        Ok(Self {
            name,
            tenant,
            ring,
            attestation_floor: Classification::Secret,
            capacity: Capacity {
                cpu_millis: 4000,
                memory_mb: 8192,
                gpu: 0,
                max_workloads: 4,
            },
            control_plane,
            listen,
            heartbeat: Duration::from_secs(heartbeat_secs),
        })
    }
}

/// A running edge node daemon: registers + heartbeats its `Node` with the control
/// plane and serves `/healthz`.
pub struct NodeAgent {
    client: Client,
    node: Node,
    key: String,
    heartbeat: Duration,
}

impl NodeAgent {
    /// Build the agent from `config`: an `aogd` client to the control plane and the
    /// `Node` the daemon will register.
    #[must_use]
    pub fn new(config: NodeConfig) -> Self {
        let mut node: Node = Resource::new(
            config.name.as_str(),
            NodeSpec {
                ring: config.ring,
                attestation_floor: config.attestation_floor,
                attestation: AttestationProfile::default(),
                capacity: config.capacity,
            },
        );
        let key = format!("Node/{}", config.name);
        node.metadata.tenant = Some(config.tenant);
        Self {
            client: Client::new(config.control_plane),
            node,
            key,
            heartbeat: config.heartbeat,
        }
    }

    /// Report the node to the control plane: write its `Node` with a fresh `Ready`
    /// heartbeat. Idempotent (last-writer-wins on its own key) — the first call is
    /// registration, every later call a heartbeat.
    ///
    /// # Errors
    /// [`NodedError`] if the `Node` cannot be encoded or the control plane refuses
    /// the write.
    pub async fn report(&self) -> Result<(), NodedError> {
        let mut node = self.node.clone();
        node.status = Some(heartbeat(self.node.spec.capacity, Utc::now()));
        let value = serde_json::to_vec(&node)?;
        self.client
            .write(Op::Put {
                key: self.key.clone(),
                value,
                expected: Precondition::Any,
            })
            .await?;
        Ok(())
    }

    /// Register, then heartbeat on the configured interval while serving `/healthz`
    /// on `listener` until the server stops.
    ///
    /// # Errors
    /// [`NodedError`] if registration fails or the health server errors.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> Result<(), NodedError> {
        self.report().await?;
        let beat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(self.heartbeat).await;
                if let Err(e) = self.report().await {
                    tracing::warn!(error = %e, "node heartbeat failed");
                }
            }
        });
        let result = axum::serve(listener, health_router()).await;
        beat.abort();
        result.map_err(NodedError::Io)
    }
}

async fn healthz() -> &'static str {
    "ok"
}

fn health_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}
