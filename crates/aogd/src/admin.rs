//! The VH2 admin API — the thin control surface the conformance harness drives a
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
use std::sync::Arc;

use aog_store::raft::RaftNode;
use aog_store::raft::types::{NodeId, RaftResponse};
use aog_store::{Op, Versioned};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use openraft::BasicNode;

use crate::api::{ChangeMembershipRequest, GetRequest, InitializeRequest, LeaderStatus, Member};

type AdminResult<T> = Result<T, (StatusCode, String)>;

/// Marks a write already forwarded once, so the leader hop cannot loop.
const FORWARDED_HEADER: &str = "x-loom-forwarded";

/// Shared admin state: the Raft node plus an HTTP client for forwarding writes to
/// the current leader.
#[derive(Clone)]
pub struct AdminState {
    node: Arc<RaftNode>,
    http: reqwest::Client,
}

/// The admin + health routes, backed by `node`.
pub fn router(node: Arc<RaftNode>) -> Router {
    let state = AdminState {
        node,
        http: reqwest::Client::new(),
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/admin/initialize", post(initialize))
        .route("/admin/add-learner", post(add_learner))
        .route("/admin/change-membership", post(change_membership))
        .route("/admin/write", post(write))
        .route("/admin/get", post(get_key))
        .route("/admin/leader", get(leader))
        .with_state(state)
}

/// Map any node/raft failure to a 500 carrying its reason (fail-closed: the harness
/// sees the error, never a silent success).
fn failed(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The URL of the current leader (from the Raft membership), if one is established.
fn leader_url(node: &RaftNode) -> Option<String> {
    let raft = node.raft();
    let metrics = raft.metrics();
    let m = metrics.borrow();
    let leader = m.current_leader?;
    m.membership_config
        .membership()
        .get_node(&leader)
        .map(|n| n.addr.clone())
}

async fn healthz() -> &'static str {
    "ok"
}

/// Form a fresh cluster with the given members (ids + peer URLs).
async fn initialize(
    State(state): State<AdminState>,
    Json(req): Json<InitializeRequest>,
) -> AdminResult<StatusCode> {
    let members: BTreeMap<NodeId, BasicNode> = req
        .members
        .into_iter()
        .map(|m| (m.id, BasicNode::new(m.addr)))
        .collect();
    state
        .node
        .raft()
        .initialize(members)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
}

/// Add a learner (non-voting) at its peer URL and wait for it to catch up.
async fn add_learner(
    State(state): State<AdminState>,
    Json(m): Json<Member>,
) -> AdminResult<StatusCode> {
    state
        .node
        .raft()
        .add_learner(m.id, BasicNode::new(m.addr), true)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
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
    let Some(url) = leader_url(node) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no leader currently known; retry".to_owned(),
        ));
    };
    let resp = state
        .http
        .post(format!("{url}/admin/write"))
        .header(FORWARDED_HEADER, "1")
        .json(&op)
        .send()
        .await
        .map_err(|e| {
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
