//! `aog-wire` (VH1) — the over-the-wire Raft transport for the containerized Loom
//! control plane. A [`WireNetwork`] `RaftNetworkFactory` sends openraft's
//! `append_entries` / `vote` / `install_snapshot` RPCs as JSON over HTTP to a
//! peer's `/raft/*` endpoints; [`router`] serves those endpoints from a node's
//! Raft handle. This is the wire counterpart of `aog-store`'s in-process
//! `ClusterNetwork` — the "deployment packaging" the plan exercises in Phase V.
//!
//! VH5b lands transport security: [`tls::NodeTls`] builds the mutually-authenticated
//! (sender-constrained, doctrine I-3) rustls configs — a raft server that requires
//! a CA-signed client certificate, and a client that presents its identity and
//! pins the estate CA. [`WireNetwork::with_tls`] carries the client leg; the plain
//! HTTP path still runs where no TLS is configured.

pub mod tls;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use aog_store::raft::RaftNode;
use aog_store::raft::types::{NodeId, TypeConfig};
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use openraft::BasicNode;
use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

// ─────────────────────────────── client ───────────────────────────────

/// A [`RaftNetworkFactory`] that reaches each peer over HTTP at the URL carried
/// in its `BasicNode` address. Cheap to clone (shares one connection pool).
#[derive(Clone, Default)]
pub struct WireNetwork {
    http: reqwest::Client,
}

impl WireNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A wire network whose peer connections use mutually-authenticated TLS
    /// (VH5b): the reqwest client presents this node's identity and verifies each
    /// peer's server certificate against the estate CA (`client_config`). Peer
    /// URLs in cluster membership must then be `https://`.
    ///
    /// # Errors
    /// [`reqwest::Error`] if the TLS-configured client cannot be built.
    pub fn with_tls(client_config: rustls::ClientConfig) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(client_config)
            .build()?;
        Ok(Self { http })
    }
}

/// A connection to one peer at `url`.
pub struct WireConnection {
    http: reqwest::Client,
    url: String,
    target: NodeId,
}

impl RaftNetworkFactory<TypeConfig> for WireNetwork {
    type Network = WireConnection;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        WireConnection {
            http: self.http.clone(),
            url: node.addr.clone(),
            target,
        }
    }
}

fn unreachable(e: impl std::fmt::Display) -> Unreachable {
    Unreachable::new(&io::Error::other(e.to_string()))
}

impl WireConnection {
    /// POST `rpc` as JSON to `path` and decode the peer's `Result<Resp, E>`. The
    /// outer `Err` is a transport failure (peer down / non-2xx / decode error) —
    /// the caller lifts it into `RPCError::Unreachable`; the inner `Err(E)` is the
    /// peer's own Raft error, lifted into `RPCError::RemoteError`.
    async fn call<Req, Resp, E>(
        &self,
        path: &str,
        rpc: &Req,
    ) -> Result<Result<Resp, E>, Unreachable>
    where
        Req: Serialize,
        Resp: DeserializeOwned,
        E: DeserializeOwned,
    {
        let body = serde_json::to_vec(rpc).map_err(unreachable)?;
        let resp = self
            .http
            .post(format!("{}{path}", self.url))
            .body(body)
            .send()
            .await
            .map_err(unreachable)?;
        if !resp.status().is_success() {
            return Err(unreachable(format!(
                "peer {} returned {}",
                self.target,
                resp.status()
            )));
        }
        let bytes = resp.bytes().await.map_err(unreachable)?;
        serde_json::from_slice(&bytes).map_err(unreachable)
    }
}

impl RaftNetwork<TypeConfig> for WireConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        self.call::<_, AppendEntriesResponse<NodeId>, RaftError<NodeId>>(
            "/raft/append-entries",
            &rpc,
        )
        .await
        .map_err(RPCError::Unreachable)?
        .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        self.call::<_, InstallSnapshotResponse<NodeId>, RaftError<NodeId, InstallSnapshotError>>(
            "/raft/install-snapshot",
            &rpc,
        )
        .await
        .map_err(RPCError::Unreachable)?
        .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        self.call::<_, VoteResponse<NodeId>, RaftError<NodeId>>("/raft/vote", &rpc)
            .await
            .map_err(RPCError::Unreachable)?
            .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }
}

// ─────────────────────────────── server ───────────────────────────────

/// An axum router serving a node's Raft RPC endpoints from its handle. Mount it
/// on a listener the peers can reach; the peer URL in cluster membership points
/// here.
pub fn router(node: Arc<RaftNode>) -> Router {
    Router::new()
        .route("/raft/append-entries", post(append_entries))
        .route("/raft/vote", post(vote))
        .route("/raft/install-snapshot", post(install_snapshot))
        .with_state(node)
}

/// Bind `addr` and serve `node`'s Raft endpoints until the task is dropped.
///
/// # Errors
/// If the listener cannot bind to `addr`.
pub async fn serve(node: Arc<RaftNode>, addr: SocketAddr) -> io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(node)).await
}

async fn append_entries(
    State(node): State<Arc<RaftNode>>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    let rpc: AppendEntriesRequest<TypeConfig> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = node.raft().append_entries(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn vote(State(node): State<Arc<RaftNode>>, body: Bytes) -> Result<Vec<u8>, StatusCode> {
    let rpc: VoteRequest<NodeId> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = node.raft().vote(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn install_snapshot(
    State(node): State<Arc<RaftNode>>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    let rpc: InstallSnapshotRequest<TypeConfig> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = node.raft().install_snapshot(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
