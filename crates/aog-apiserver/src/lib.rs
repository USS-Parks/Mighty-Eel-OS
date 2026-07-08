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
pub mod backup;
pub mod codec;
pub mod convert;
pub mod error;
pub mod handlers;
pub mod policy;
pub mod reader;
pub mod seal;

use std::path::Path;
use std::sync::Arc;

use axum::routing::get;
use axum::{Router, middleware};

use aog_store::raft::types::NodeId;
use aog_store::raft::{NodeError, RaftNode};
use wsf_ledger::EvidencePack;

use crate::admission::Admission;
use crate::auth::Authenticator;
use crate::convert::ConversionRegistry;
use crate::reader::StoreReader;
use crate::seal::Sealer;

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
        sealer: Sealer,
    ) -> Result<Self, NodeError> {
        let raft = Arc::new(RaftNode::bootstrap(node_id, dir).await?);
        Ok(Self::from_raft(raft, authenticator, sealer))
    }

    /// Recover an existing estate under `dir` (no cluster init).
    ///
    /// # Errors
    /// [`NodeError`] if the Raft node cannot start.
    pub async fn start(
        node_id: NodeId,
        dir: impl AsRef<Path>,
        authenticator: Authenticator,
        sealer: Sealer,
    ) -> Result<Self, NodeError> {
        let raft = Arc::new(RaftNode::start(node_id, dir).await?);
        Ok(Self::from_raft(raft, authenticator, sealer))
    }

    /// Wrap an already-running Raft node in authenticated API state — the VH5b
    /// seam. It lets the wire-transport control-plane daemon (`aogd`) serve the
    /// authenticated CRUD surface over the very node it drives consensus on,
    /// rather than a separately bootstrapped one.
    #[must_use]
    pub fn from_raft(raft: Arc<RaftNode>, authenticator: Authenticator, sealer: Sealer) -> Self {
        Self {
            admission: Arc::new(Admission::new(Arc::clone(&raft), sealer)),
            reader: StoreReader::new(raft),
            authenticator: Arc::new(authenticator),
        }
    }

    /// Number of admitted-mutation receipts in the ledger (K9).
    #[must_use]
    pub fn receipts_len(&self) -> usize {
        self.admission.receipts_len()
    }

    /// The receipt ledger's public key — verifies an exported pack off-host.
    #[must_use]
    pub fn receipts_public_key(&self) -> Vec<u8> {
        self.admission.receipts_public_key()
    }

    /// Export a signed evidence pack over the receipt chain.
    ///
    /// # Errors
    /// [`crate::error::ApiError`] on hashing/signing failure.
    pub fn export_receipts(
        &self,
        generated_at: &str,
    ) -> Result<EvidencePack, crate::error::ApiError> {
        self.admission.export_receipts(generated_at)
    }

    /// Configure the read-path conversion registry (K10). Default is the identity
    /// (serve stored objects unchanged).
    #[must_use]
    pub fn with_conversions(mut self, conversions: ConversionRegistry) -> Self {
        self.reader = self.reader.with_conversions(conversions);
        self
    }

    /// The admission choke point, for in-process controllers (Phase R). Handing
    /// this out is safe by construction: `Admission::admit` *is* the full chain
    /// (validate → mutate → commit → receipt) — there is no writable store
    /// handle to leak (the K5 invariant).
    #[must_use]
    pub fn admission(&self) -> Arc<Admission> {
        Arc::clone(&self.admission)
    }

    /// The read-only estate view, for controllers.
    #[must_use]
    pub fn reader(&self) -> StoreReader {
        self.reader.clone()
    }

    /// The front-door authenticator (shared), for controllers that maintain its
    /// live revocation view.
    #[must_use]
    pub fn authenticator(&self) -> Arc<Authenticator> {
        Arc::clone(&self.authenticator)
    }

    /// A prefix-scoped estate informer (K4) — a controller's wakeup stream.
    #[must_use]
    pub fn informer(&self, prefix: impl Into<String>) -> aog_store::raft::watch::Informer {
        self.reader.informer(prefix)
    }
}

/// The authenticated CRUD surface alone: the `/apis/**` routes wrapped by the K6
/// authentication middleware, with `state` applied. Split out so a host that has
/// its own health/liveness surface — the wire-transport daemon `aogd` (VH5b) —
/// can merge just the authenticated CRUD over its own node without a `/healthz`
/// route collision.
#[allow(clippy::needless_pass_by_value)]
pub fn api_router(state: AppState) -> Router {
    Router::new()
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
        ))
        .with_state(state)
}

/// Build the control-plane router over shared [`AppState`]. The `/apis/**` routes
/// are wrapped by the K6 authentication middleware; `/healthz` and `/readyz` stay
/// open (unauthenticated liveness/readiness).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api_router(state))
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
