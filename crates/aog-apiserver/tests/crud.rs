//! K5 CRUD surface, now behind K6 authentication: create -> get -> list ->
//! update -> delete round-trips + conflict / not-found / bad-request edges, all
//! carrying a valid trust token.

mod common;

use axum::http::StatusCode;
use common::{BASE, authed_app, bundle, send};

#[tokio::test]
async fn crud_roundtrip() {
    let (app, tok) = authed_app("aog-apiserver-k5-crud").await;
    let t = Some(tok.as_str());

    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bundle("base", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {created}");
    assert_eq!(created["spec"]["version"], 1);
    assert_eq!(created["metadata"]["generation"], 1);
    assert!(
        created["metadata"]["uid"]
            .as_str()
            .is_some_and(|u| !u.is_empty())
    );
    assert!(created["metadata"]["resource_version"].as_u64().unwrap() >= 1);
    // K6: the object is stamped with the authenticating token's ref (provenance).
    assert_eq!(created["metadata"]["token_ref"]["token_id"], "tok-loom");

    let (status, got) = send(&app, "GET", &format!("{BASE}/PolicyBundle/base"), t, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["spec"]["version"], 1);

    let (status, listed) = send(&app, "GET", &format!("{BASE}/PolicyBundle"), t, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, updated) = send(
        &app,
        "PUT",
        &format!("{BASE}/PolicyBundle/base"),
        t,
        Some(bundle("base", 2)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update body: {updated}");
    assert_eq!(updated["spec"]["version"], 2);
    assert_eq!(updated["metadata"]["generation"], 2);

    let (status, _) = send(
        &app,
        "DELETE",
        &format!("{BASE}/PolicyBundle/base"),
        t,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(&app, "GET", &format!("{BASE}/PolicyBundle/base"), t, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_create_conflicts() {
    let (app, tok) = authed_app("aog-apiserver-k5-conflict").await;
    let t = Some(tok.as_str());
    let uri = format!("{BASE}/PolicyBundle");
    let (s1, _) = send(&app, "POST", &uri, t, Some(bundle("dup", 1))).await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, _) = send(&app, "POST", &uri, t, Some(bundle("dup", 1))).await;
    assert_eq!(s2, StatusCode::CONFLICT);
}

#[tokio::test]
async fn update_missing_is_not_found() {
    let (app, tok) = authed_app("aog-apiserver-k5-updatemiss").await;
    let (status, _) = send(
        &app,
        "PUT",
        &format!("{BASE}/PolicyBundle/ghost"),
        Some(&tok),
        Some(bundle("ghost", 2)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_kind_is_bad_request() {
    let (app, tok) = authed_app("aog-apiserver-k5-unknownkind").await;
    let (status, _) = send(&app, "GET", &format!("{BASE}/Bogus"), Some(&tok), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn kind_mismatch_is_bad_request() {
    let (app, tok) = authed_app("aog-apiserver-k5-kindmismatch").await;
    let body = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "ToolGrant",
        "metadata": { "name": "x" },
        "spec": { "tool": "github" },
    });
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(&tok),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
