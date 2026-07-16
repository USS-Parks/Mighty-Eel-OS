//! The admin API — the thin control surface the conformance harness drives a
//! daemon through. Membership operations carry each peer's real URL (the wire
//! transport reaches a peer by the address in its `BasicNode`), so they are issued
//! against the raw openraft handle rather than the [`RaftNode`] membership wrappers
//! (which address peers by an empty, id-only node — correct only in-process).
//!
//! Writes are **leader-transparent**: only the leader can commit, so a follower
//! forwards a write one hop to the current leader (looked up from the Raft
//! membership) rather than refusing it. A client may therefore write to any node —
//! it need not track which node is leader (the fix for edges heartbeating to a
//! node that later lost leadership). The hop is guarded so it cannot loop.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aog_apiserver::auth::{Authenticator, TOKEN_HEADER};
use aog_store::raft::RaftNode;
use aog_store::raft::types::{NodeId, RaftResponse};
use aog_store::{Op, Versioned};
use aog_wire::canonical_peer_origin;
use aog_wire::tls::TlsPeer;
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use openraft::BasicNode;

use crate::api::{ChangeMembershipRequest, GetRequest, InitializeRequest, LeaderStatus, Member};

type AdminResult<T> = Result<T, (StatusCode, String)>;

/// Marks a write already forwarded once, so the leader hop cannot loop.
const FORWARDED_HEADER: &str = "x-loom-forwarded";

/// The trust role a WSF token must carry to use the mutating admin API (A1).
pub const AOG_ADMIN_ROLE: &str = "aog-admin";

/// Shared admin state: the Raft node plus an HTTP client for forwarding writes to
/// the current leader.
#[derive(Clone)]
pub struct AdminState {
    node: Arc<RaftNode>,
    http: reqwest::Client,
    /// The front-door authenticator, present once a trust anchor is provisioned.
    /// When present, the mutating `/admin/*` routes require an admin-scoped token;
    /// when absent (pre-anchor bootstrap) the daemon is loopback-contained (0.2).
    authenticator: Option<Arc<Authenticator>>,
    /// When true, every membership address must be HTTPS and forwarding uses
    /// the node's mutually-authenticated client.
    secure_transport: bool,
    /// Explicit legacy harness mode, rejected by the production binary.
    allow_insecure_admin: bool,
    /// One-shot local initialization capability when trust is absent.
    bootstrap_open: Arc<AtomicBool>,
}

/// The admin + health routes, backed by `node`.
pub fn router(
    node: Arc<RaftNode>,
    authenticator: Option<Arc<Authenticator>>,
    http: reqwest::Client,
    secure_transport: bool,
    allow_insecure_admin: bool,
) -> Router {
    let has_membership = node
        .raft()
        .metrics()
        .borrow()
        .membership_config
        .membership()
        .voter_ids()
        .next()
        .is_some();
    let state = AdminState {
        node,
        http,
        authenticator,
        secure_transport,
        allow_insecure_admin,
        bootstrap_open: Arc::new(AtomicBool::new(!has_membership)),
    };
    // A1 (audit C1): the mutating admin routes require an admin-scoped principal once
    // an anchor is provisioned; `/healthz` and the read-only `/admin/leader` stay open.
    // The pre-anchor bootstrap is guarded by the 0.2 loopback containment.
    let guarded = Router::new()
        .route("/admin/initialize", post(initialize))
        .route("/admin/add-learner", post(add_learner))
        .route("/admin/change-membership", post(change_membership))
        .route("/admin/write", post(write))
        .route("/admin/get", post(get_key))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_admin,
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/admin/leader", get(leader))
        .merge(guarded)
        .with_state(state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

/// A1 middleware: authenticate an admin request against the front-door authenticator
/// (when provisioned) and require the `aog-admin` role. No authenticator means the
/// pre-anchor bootstrap posture, which the 0.2 loopback containment gates.
async fn require_admin(
    State(state): State<AdminState>,
    req: Request,
    next: Next,
) -> AdminResult<Response> {
    if req.headers().contains_key(FORWARDED_HEADER) {
        if !state.secure_transport {
            return Err((
                StatusCode::FORBIDDEN,
                "forwarded admin writes require authenticated node transport".to_owned(),
            ));
        }
        if req.uri().path() != "/admin/write" || req.headers().contains_key(TOKEN_HEADER) {
            return Err((
                StatusCode::FORBIDDEN,
                "forwarded admin request has an invalid path or carries a bearer token".to_owned(),
            ));
        }
        let peer = request_tls_peer(req.extensions()).ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "forwarded admin write requires a node certificate".to_owned(),
            )
        })?;
        let peer_id = peer.node_id.ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "forwarded admin write requires a node SPIFFE identity".to_owned(),
            )
        })?;
        if !is_current_member(&state, peer_id) {
            return Err((
                StatusCode::FORBIDDEN,
                "forwarded admin write came from a non-member node".to_owned(),
            ));
        }
        return Ok(next.run(req).await);
    }
    if let Some(auth) = &state.authenticator {
        let principal = auth.authenticate(req.headers()).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "admin trust token required".to_owned(),
            )
        })?;
        let is_admin = principal
            .token()
            .as_ref()
            .is_some_and(|t| roles_include_admin(&t.roles));
        if !is_admin {
            return Err((
                StatusCode::FORBIDDEN,
                format!("admin role '{AOG_ADMIN_ROLE}' required"),
            ));
        }
    } else if !state.allow_insecure_admin {
        let peer = request_peer(req.extensions());
        if !bounded_bootstrap_allows(
            req.uri().path(),
            peer,
            state.bootstrap_open.load(Ordering::Acquire),
        ) {
            return Err((
                StatusCode::FORBIDDEN,
                "admin trust is not provisioned; only one local initialize is permitted".to_owned(),
            ));
        }
    }
    Ok(next.run(req).await)
}

fn request_peer(extensions: &axum::http::Extensions) -> SocketAddr {
    request_tls_peer(extensions)
        .map(|peer| peer.socket_addr)
        .or_else(|| {
            extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(peer)| *peer)
        })
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)))
}

fn request_tls_peer(extensions: &axum::http::Extensions) -> Option<TlsPeer> {
    extensions
        .get::<ConnectInfo<TlsPeer>>()
        .map(|ConnectInfo(peer)| *peer)
}

fn bounded_bootstrap_allows(path: &str, peer: SocketAddr, bootstrap_open: bool) -> bool {
    bootstrap_open && peer.ip().is_loopback() && path == "/admin/initialize"
}

/// Whether a token's roles include the admin role.
fn roles_include_admin(roles: &[String]) -> bool {
    roles.iter().any(|r| r == AOG_ADMIN_ROLE)
}

/// Map any node/raft failure to a 500 carrying its reason (fail-closed: the harness
/// sees the error, never a silent success).
fn failed(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The pinned node id and URL of the current leader, if one is established.
fn leader_binding(node: &RaftNode) -> Option<(NodeId, String)> {
    let raft = node.raft();
    let metrics = raft.metrics();
    let m = metrics.borrow();
    let leader = m.current_leader?;
    m.membership_config
        .membership()
        .get_node(&leader)
        .map(|n| (leader, n.addr.clone()))
}

fn current_member_bindings(state: &AdminState) -> Vec<(NodeId, String)> {
    let metrics = state.node.raft().metrics();
    let metrics = metrics.borrow();
    let membership = metrics.membership_config.membership();
    membership
        .voter_ids()
        .chain(membership.learner_ids())
        .filter_map(|id| membership.get_node(&id).map(|node| (id, node.addr.clone())))
        .collect()
}

fn is_current_member(state: &AdminState, node_id: NodeId) -> bool {
    current_member_bindings(state)
        .iter()
        .any(|(id, _)| *id == node_id)
}

async fn healthz() -> &'static str {
    "ok"
}

/// Form a fresh cluster with the given members (ids + peer URLs).
async fn initialize(
    State(state): State<AdminState>,
    Json(req): Json<InitializeRequest>,
) -> AdminResult<StatusCode> {
    let bounded_bootstrap = state.authenticator.is_none() && !state.allow_insecure_admin;
    if bounded_bootstrap
        && state
            .bootstrap_open
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err((
            StatusCode::CONFLICT,
            "local bootstrap has already been consumed".to_owned(),
        ));
    }
    let mut members = BTreeMap::new();
    let mut origins = BTreeMap::new();
    for member in req.members {
        let origin = validate_member_binding(&state, member.id, &member.addr)?;
        if members.contains_key(&member.id) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate membership node id {}", member.id),
            ));
        }
        if let Some(bound_id) = origins.insert(origin.clone(), member.id) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("membership origin is already pinned to node {bound_id}"),
            ));
        }
        members.insert(member.id, BasicNode::new(origin));
    }
    let result = state.node.raft().initialize(members).await;
    if let Err(error) = result {
        if bounded_bootstrap {
            state.bootstrap_open.store(true, Ordering::Release);
        }
        return Err(failed(error));
    }
    Ok(StatusCode::OK)
}

/// Add a learner (non-voting) at its peer URL and wait for it to catch up.
async fn add_learner(
    State(state): State<AdminState>,
    Json(m): Json<Member>,
) -> AdminResult<StatusCode> {
    let origin = validate_member_binding(&state, m.id, &m.addr)?;
    state
        .node
        .raft()
        .add_learner(m.id, BasicNode::new(origin), true)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
}

fn validate_member_binding(
    state: &AdminState,
    node_id: NodeId,
    address: &str,
) -> AdminResult<String> {
    let origin = canonical_peer_origin(address, state.secure_transport)
        .map_err(|reason| (StatusCode::BAD_REQUEST, reason))?;
    for (bound_id, bound_address) in current_member_bindings(state) {
        let bound_origin =
            canonical_peer_origin(&bound_address, state.secure_transport).map_err(|reason| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("stored membership binding is invalid: {reason}"),
                )
            })?;
        if bound_id == node_id && bound_origin != origin {
            return Err((
                StatusCode::CONFLICT,
                format!("node {node_id} is already pinned to a different origin"),
            ));
        }
        if bound_id != node_id && bound_origin == origin {
            return Err((
                StatusCode::CONFLICT,
                format!("membership origin is already pinned to node {bound_id}"),
            ));
        }
    }
    Ok(origin)
}

/// Set the cluster's voter set (promotes caught-up learners, or removes a member).
async fn change_membership(
    State(state): State<AdminState>,
    Json(req): Json<ChangeMembershipRequest>,
) -> AdminResult<StatusCode> {
    let voters: BTreeSet<NodeId> = req.voters.into_iter().collect();
    state
        .node
        .raft()
        .change_membership(voters, false)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
}

/// Linearizably apply one desired-state mutation. On the leader it commits locally;
/// on a follower it forwards one hop to the current leader (a client may write to
/// any node).
async fn write(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(op): Json<Op>,
) -> AdminResult<Json<RaftResponse>> {
    let node = &state.node;
    if node.current_leader() == Some(node.id()) {
        return Ok(Json(node.write(op).await.map_err(failed)?));
    }
    // Reached a non-leader on a write already forwarded once: don't hop again.
    if headers.contains_key(FORWARDED_HEADER) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "forwarded but this node is not the leader; retry".to_owned(),
        ));
    }
    let Some((leader_id, url)) = leader_binding(node) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no leader currently known; retry".to_owned(),
        ));
    };
    let url = validate_member_binding(&state, leader_id, &url)?;
    // The follower already authenticated the caller. The leader authenticates the
    // forwarding node through mTLS membership instead of sending the caller's
    // bearer token to a membership-selected network destination.
    let fwd = forwarded_write_request(&state.http, &url, &op);
    let resp = fwd.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("forwarding to leader failed: {e}"),
        )
    })?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if !status.is_success() {
        return Err((status, resp.text().await.unwrap_or_default()));
    }
    resp.json::<RaftResponse>()
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))
}

fn forwarded_write_request(http: &reqwest::Client, url: &str, op: &Op) -> reqwest::RequestBuilder {
    http.post(format!("{url}/admin/write"))
        .header(FORWARDED_HEADER, "1")
        .json(op)
}

/// Read one applied key from this node's committed state.
async fn get_key(
    State(state): State<AdminState>,
    Json(req): Json<GetRequest>,
) -> AdminResult<Json<Option<Versioned>>> {
    let value = state.node.get(&req.key).await.map_err(failed)?;
    Ok(Json(value))
}

/// This node's id and its view of the current leader.
async fn leader(State(state): State<AdminState>) -> Json<LeaderStatus> {
    Json(LeaderStatus {
        id: state.node.id(),
        leader: state.node.current_leader(),
        is_leader: state.node.is_leader(),
    })
}

#[cfg(test)]
mod admin_auth_tests {
    use std::net::SocketAddr;

    use aog_apiserver::auth::TOKEN_HEADER;
    use aog_store::{Op, Precondition};

    use super::{
        AOG_ADMIN_ROLE, FORWARDED_HEADER, bounded_bootstrap_allows, forwarded_write_request,
        roles_include_admin,
    };

    #[test]
    fn admin_role_gate() {
        assert!(roles_include_admin(&[
            "x".to_owned(),
            AOG_ADMIN_ROLE.to_owned()
        ]));
        assert!(!roles_include_admin(&["adult".to_owned()]));
        assert!(!roles_include_admin(&[]));
    }

    #[test]
    fn bootstrap_is_one_time_initialize_from_loopback_only() {
        let local: SocketAddr = "127.0.0.1:4444".parse().unwrap();
        let remote: SocketAddr = "192.0.2.44:4444".parse().unwrap();
        assert!(bounded_bootstrap_allows("/admin/initialize", local, true));
        assert!(!bounded_bootstrap_allows("/admin/write", local, true));
        assert!(!bounded_bootstrap_allows("/admin/initialize", remote, true));
        assert!(!bounded_bootstrap_allows("/admin/initialize", local, false));
    }

    #[test]
    fn internal_forwarding_request_never_carries_caller_credentials() {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let request = forwarded_write_request(
            &http,
            "https://leader.example:4600",
            &Op::Delete {
                key: "Workload/test".to_owned(),
                expected: Precondition::Any,
            },
        )
        .build()
        .unwrap();
        assert_eq!(request.headers()[FORWARDED_HEADER], "1");
        assert!(!request.headers().contains_key(TOKEN_HEADER));
        assert!(!request.headers().contains_key("authorization"));
    }
}
