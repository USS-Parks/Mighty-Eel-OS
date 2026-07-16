//! `aog-wire` — the over-the-wire Raft transport for the containerized Loom
//! control plane. A [`WireNetwork`] `RaftNetworkFactory` sends openraft's
//! `append_entries` / `vote` / `install_snapshot` RPCs as JSON over HTTP to a
//! peer's `/raft/*` endpoints; [`router`] serves those endpoints from a node's
//! Raft handle. This is the wire counterpart of `aog-store`'s in-process
//! `ClusterNetwork` — the "deployment packaging" the plan exercises in Phase V.
//!
//! Transport security: [`tls::NodeTls`] builds the mutually-authenticated
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
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
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

use crate::tls::TlsPeer;

// ─────────────────────────────── client ───────────────────────────────

/// A [`RaftNetworkFactory`] that reaches each peer over HTTP at the URL carried
/// in its `BasicNode` address. Cheap to clone (shares one connection pool).
#[derive(Clone)]
pub struct WireNetwork {
    http: reqwest::Client,
    require_https: bool,
}

impl Default for WireNetwork {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("redirect-disabled Raft client construction");
        Self {
            http,
            require_https: false,
        }
    }
}

impl WireNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A wire network whose peer connections use mutually-authenticated TLS:
    /// the reqwest client presents this node's identity and verifies each
    /// peer's server certificate against the estate CA (`client_config`). Peer
    /// URLs in cluster membership must then be `https://`.
    ///
    /// # Errors
    /// [`reqwest::Error`] if the TLS-configured client cannot be built.
    pub fn with_tls(client_config: rustls::ClientConfig) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(client_config)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            require_https: true,
        })
    }
}

/// Parse an untrusted membership address into the only origin form the wire
/// transport will use. Membership entries are origins, never general URLs: no
/// credentials, paths, query strings, fragments, unknown schemes, or port zero.
/// The returned value is normalized for stable node-id/address pinning.
///
/// # Errors
/// Returns a public, credential-free reason when the address is not a valid
/// control-plane origin.
pub fn canonical_peer_origin(address: &str, require_https: bool) -> Result<String, String> {
    if address.trim() != address || address.len() > 2_048 {
        return Err("membership address must be a bounded origin without whitespace".to_owned());
    }
    let url = reqwest::Url::parse(address).map_err(|e| format!("invalid member URI: {e}"))?;
    let allowed_scheme = if require_https {
        url.scheme() == "https"
    } else {
        matches!(url.scheme(), "http" | "https")
    };
    if !allowed_scheme {
        return Err(if require_https {
            "secure Raft membership requires https:// addresses".to_owned()
        } else {
            "Raft membership requires an http:// or https:// address".to_owned()
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("membership address must not contain credentials".to_owned());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(
            "membership address must be an origin without path, query, or fragment".to_owned(),
        );
    }
    if url.host_str().is_none() {
        return Err("membership address must contain a host".to_owned());
    }
    if url.port_or_known_default().is_none() || url.port() == Some(0) {
        return Err("membership address must contain a valid TCP port".to_owned());
    }
    Ok(url.origin().ascii_serialization())
}

/// A connection to one peer at `url`.
pub struct WireConnection {
    http: reqwest::Client,
    url: Result<String, String>,
    target: NodeId,
}

impl RaftNetworkFactory<TypeConfig> for WireNetwork {
    type Network = WireConnection;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        WireConnection {
            http: self.http.clone(),
            url: canonical_peer_origin(&node.addr, self.require_https),
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
        let url = self.url.as_ref().map_err(unreachable)?;
        let body = serde_json::to_vec(rpc).map_err(unreachable)?;
        let resp = self
            .http
            .post(format!("{url}{path}"))
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
    router_with_peer_identity(node, false)
}

/// A Raft router that requires the mTLS certificate's node SPIFFE identity to
/// match the sender id carried by each decoded RPC before invoking openraft.
pub fn secure_router(node: Arc<RaftNode>) -> Router {
    router_with_peer_identity(node, true)
}

#[derive(Clone)]
struct WireState {
    node: Arc<RaftNode>,
    require_peer_identity: bool,
}

fn router_with_peer_identity(node: Arc<RaftNode>, require_peer_identity: bool) -> Router {
    Router::new()
        .route("/raft/append-entries", post(append_entries))
        .route("/raft/vote", post(vote))
        .route("/raft/install-snapshot", post(install_snapshot))
        .with_state(WireState {
            node,
            require_peer_identity,
        })
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(MockConnectInfo(TlsPeer {
            socket_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            node_id: None,
        }))
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
    State(state): State<WireState>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    let rpc: AppendEntriesRequest<TypeConfig> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    authorize_peer(&state, peer, rpc.vote.leader_id.node_id)?;
    let result = state.node.raft().append_entries(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn vote(
    State(state): State<WireState>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    let rpc: VoteRequest<NodeId> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    authorize_peer(&state, peer, rpc.vote.leader_id.node_id)?;
    let result = state.node.raft().vote(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn install_snapshot(
    State(state): State<WireState>,
    ConnectInfo(peer): ConnectInfo<TlsPeer>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    let rpc: InstallSnapshotRequest<TypeConfig> =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    authorize_peer(&state, peer, rpc.vote.leader_id.node_id)?;
    let result = state.node.raft().install_snapshot(rpc).await;
    serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn authorize_peer(
    state: &WireState,
    peer: TlsPeer,
    claimed_node_id: NodeId,
) -> Result<(), StatusCode> {
    if !state.require_peer_identity {
        return Ok(());
    }
    match peer.node_id {
        Some(authenticated_node_id) if authenticated_node_id == claimed_node_id => Ok(()),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

#[cfg(test)]
mod destination_policy_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::response::Redirect;
    use axum::routing::post;
    use serde_json::json;

    use super::{WireConnection, WireNetwork, canonical_peer_origin};

    #[test]
    fn membership_origins_are_canonical_and_bounded() {
        assert_eq!(
            canonical_peer_origin("https://EXAMPLE.com:443", true).unwrap(),
            "https://example.com"
        );
        assert_eq!(
            canonical_peer_origin("https://[::1]:4600/", true).unwrap(),
            "https://[::1]:4600"
        );

        for address in [
            "http://cp1:4600",
            "https://user:secret@cp1:4600",
            "https://cp1:4600/admin",
            "https://cp1:4600/?next=https://evil.test",
            "https://cp1:4600/#fragment",
            "https://cp1:0",
            " https://cp1:4600",
        ] {
            assert!(
                canonical_peer_origin(address, true).is_err(),
                "accepted unsafe membership address {address}"
            );
        }
    }

    #[tokio::test]
    async fn raft_client_never_follows_membership_redirects() {
        let redirected_requests = Arc::new(AtomicUsize::new(0));
        let sink_count = Arc::clone(&redirected_requests);
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sink_url = format!("http://{}", sink.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(
                sink,
                Router::new().route(
                    "/raft/vote",
                    post(move || async move {
                        sink_count.fetch_add(1, Ordering::SeqCst);
                        "followed"
                    }),
                ),
            )
            .await
            .unwrap();
        });

        let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_url = format!("http://{}", redirect.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(
                redirect,
                Router::new().route(
                    "/raft/vote",
                    post(move || {
                        let location = format!("{sink_url}/raft/vote");
                        async move { Redirect::temporary(&location) }
                    }),
                ),
            )
            .await
            .unwrap();
        });

        let network = WireNetwork::new();
        let connection = WireConnection {
            http: network.http,
            url: Ok(redirect_url),
            target: 2,
        };
        let result = connection
            .call::<_, serde_json::Value, serde_json::Value>("/raft/vote", &json!({}))
            .await;
        assert!(result.is_err(), "redirect response must fail closed");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(redirected_requests.load(Ordering::SeqCst), 0);
    }
}
