//! `aog-apiserver` — the Loom control-plane API server (K5–K6…).
//!
//! A typed CRUD surface over the estate kinds (`aog-estate`) backed by the
//! consensus store (`aog-store`). Two invariants this crate exists to hold:
//! **every request is authenticated at the front door** ([`auth`], K6), and
//! **every desired-state mutation traverses the admission chain — no handler can
//! write the store directly** (enforced by type; see [`admission::Admission`],
//! whose private `RaftNode` handle is the only writer, paired with a read-only
//! [`reader::StoreReader`]; and by test in `tests/admission_bypass.rs`).
//!
//! AuthN is live at the front door (K6). Policy deny-wins (K7), envelope-seal +
//! token attenuation (K8), and receipt binding (K9) are the remaining named
//! seams in the admission chain.

pub mod admission;
pub mod auth;
pub mod codec;
pub mod error;
pub mod handlers;
pub mod policy;
pub mod reader;

use std::path::Path;
use std::sync::Arc;

use axum::routing::get;
use axum::{Router, middleware};

use aog_store::raft::types::NodeId;
use aog_store::raft::{NodeError, RaftNode};

use crate::admission::Admission;
use crate::auth::Authenticator;
use crate::reader::StoreReader;

/// The API group + version every route is served under.
pub const API_GROUP_VERSION: &str = "aog.islandmountain.io/v1";

/// Shared server state: the front-door authenticator, the admission writer, and
/// the read-only view. Cheap to clone (all `Arc`-backed). A handler is handed
/// exactly these capabilities — an authenticated request, a write path that *is*
/// the admission chain, and a read path that cannot write.
#[derive(Clone)]
pub struct AppState {
    pub(crate) admission: Arc<Admission>,
    pub(crate) reader: StoreReader,
    pub(crate) authenticator: Arc<Authenticator>,
}

impl AppState {
    /// Bootstrap a fresh single-node estate under `dir`, anchored on
    /// `authenticator`, and assemble state.
    ///
    /// # Errors
    /// [`NodeError`] if the Raft node cannot bootstrap.
    pub async fn bootstrap(
        node_id: NodeId,
        dir: impl AsRef<Path>,
        authenticator: Authenticator,
    ) -> Result<Self, NodeError> {
        let raft = Arc::new(RaftNode::bootstrap(node_id, dir).await?);
        Ok(Self::from_node(raft, authenticator))
    }

    /// Recover an existing estate under `dir` (no cluster init).
    ///
    /// # Errors
    /// [`NodeError`] if the Raft node cannot start.
    pub async fn start(
        node_id: NodeId,
        dir: impl AsRef<Path>,
        authenticator: Authenticator,
    ) -> Result<Self, NodeError> {
        let raft = Arc::new(RaftNode::start(node_id, dir).await?);
        Ok(Self::from_node(raft, authenticator))
    }

    fn from_node(raft: Arc<RaftNode>, authenticator: Authenticator) -> Self {
        Self {
            admission: Arc::new(Admission::new(Arc::clone(&raft))),
            reader: StoreReader::new(raft),
            authenticator: Arc::new(authenticator),
        }
    }
}

/// Build the control-plane router over shared [`AppState`]. The `/apis/**` routes
/// are wrapped by the K6 authentication middleware; `/healthz` and `/readyz` stay
/// open (unauthenticated liveness/readiness).
#[allow(clippy::needless_pass_by_value)]
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/apis/aog.islandmountain.io/v1/{kind}",
            get(handlers::list).post(handlers::create),
        )
        .route(
            "/apis/aog.islandmountain.io/v1/{kind}/{name}",
            get(handlers::get_one)
                .put(handlers::update)
                .delete(handlers::delete),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api)
        .with_state(state)
}

/// Serve the control-plane API on `listener` until shutdown.
///
/// # Errors
/// I/O error from the underlying server.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz() -> &'static str {
    "ok"
}
