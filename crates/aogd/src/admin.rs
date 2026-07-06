//! The VH2 admin API — the thin control surface the conformance harness drives a
//! daemon through. Membership operations carry each peer's real URL (the wire
//! transport reaches a peer by the address in its `BasicNode`), so they are issued
//! against the raw openraft handle rather than the [`RaftNode`] membership wrappers
//! (which address peers by an empty, id-only node — correct only in-process).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aog_store::raft::RaftNode;
use aog_store::raft::types::{NodeId, RaftResponse};
use aog_store::{Op, Versioned};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use openraft::BasicNode;

use crate::api::{ChangeMembershipRequest, GetRequest, InitializeRequest, LeaderStatus, Member};

type AdminResult<T> = Result<T, (StatusCode, String)>;

/// The admin + health routes, stated on `node`.
pub fn router(node: Arc<RaftNode>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/admin/initialize", post(initialize))
        .route("/admin/add-learner", post(add_learner))
        .route("/admin/change-membership", post(change_membership))
        .route("/admin/write", post(write))
        .route("/admin/get", post(get_key))
        .route("/admin/leader", get(leader))
        .with_state(node)
}

/// Map any node/raft failure to a 500 carrying its reason (fail-closed: the harness
/// sees the error, never a silent success).
fn failed(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn healthz() -> &'static str {
    "ok"
}

/// Form a fresh cluster with the given members (ids + peer URLs).
async fn initialize(
    State(node): State<Arc<RaftNode>>,
    Json(req): Json<InitializeRequest>,
) -> AdminResult<StatusCode> {
    let members: BTreeMap<NodeId, BasicNode> = req
        .members
        .into_iter()
        .map(|m| (m.id, BasicNode::new(m.addr)))
        .collect();
    node.raft().initialize(members).await.map_err(failed)?;
    Ok(StatusCode::OK)
}

/// Add a learner (non-voting) at its peer URL and wait for it to catch up.
async fn add_learner(
    State(node): State<Arc<RaftNode>>,
    Json(m): Json<Member>,
) -> AdminResult<StatusCode> {
    node.raft()
        .add_learner(m.id, BasicNode::new(m.addr), true)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
}

/// Set the cluster's voter set (promotes caught-up learners, or removes a member).
async fn change_membership(
    State(node): State<Arc<RaftNode>>,
    Json(req): Json<ChangeMembershipRequest>,
) -> AdminResult<StatusCode> {
    let voters: BTreeSet<NodeId> = req.voters.into_iter().collect();
    node.raft()
        .change_membership(voters, false)
        .await
        .map_err(failed)?;
    Ok(StatusCode::OK)
}

/// Linearizably apply one desired-state mutation on the leader.
async fn write(
    State(node): State<Arc<RaftNode>>,
    Json(op): Json<Op>,
) -> AdminResult<Json<RaftResponse>> {
    let response = node.write(op).await.map_err(failed)?;
    Ok(Json(response))
}

/// Read one applied key from this node's committed state.
async fn get_key(
    State(node): State<Arc<RaftNode>>,
    Json(req): Json<GetRequest>,
) -> AdminResult<Json<Option<Versioned>>> {
    let value = node.get(&req.key).await.map_err(failed)?;
    Ok(Json(value))
}

/// This node's id and its view of the current leader.
async fn leader(State(node): State<Arc<RaftNode>>) -> Json<LeaderStatus> {
    Json(LeaderStatus {
        id: node.id(),
        leader: node.current_leader(),
        is_leader: node.is_leader(),
    })
}
