//! K5 CRUD surface: create -> get -> list -> update -> delete round-trips
//! through the real router + admission + store, plus the conflict / not-found /
//! bad-request edges. Driven in-process with `tower::ServiceExt::oneshot` (no
//! socket bound).

use std::path::PathBuf;

use aog_apiserver::{AppState, router};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "/apis/aog.islandmountain.io/v1";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app(dir_name: &str) -> Router {
    let state = AppState::bootstrap(1, fresh_dir(dir_name)).await.unwrap();
    router(state)
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn bundle(name: &str, version: u32) -> Value {
    json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": name },
        "spec": { "version": version },
    })
}

#[tokio::test]
async fn crud_roundtrip() {
    let app = app("aog-apiserver-k5-crud").await;

    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
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

    let (status, got) = send(&app, "GET", &format!("{BASE}/PolicyBundle/base"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["spec"]["version"], 1);

    let (status, listed) = send(&app, "GET", &format!("{BASE}/PolicyBundle"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, updated) = send(
        &app,
        "PUT",
        &format!("{BASE}/PolicyBundle/base"),
        Some(bundle("base", 2)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update body: {updated}");
    assert_eq!(updated["spec"]["version"], 2);
    assert_eq!(updated["metadata"]["generation"], 2);

    let (status, _) = send(&app, "DELETE", &format!("{BASE}/PolicyBundle/base"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(&app, "GET", &format!("{BASE}/PolicyBundle/base"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_create_conflicts() {
    let app = app("aog-apiserver-k5-conflict").await;
    let uri = format!("{BASE}/PolicyBundle");
    let (s1, _) = send(&app, "POST", &uri, Some(bundle("dup", 1))).await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, _) = send(&app, "POST", &uri, Some(bundle("dup", 1))).await;
    assert_eq!(s2, StatusCode::CONFLICT);
}

#[tokio::test]
async fn update_missing_is_not_found() {
    let app = app("aog-apiserver-k5-updatemiss").await;
    let (status, _) = send(
        &app,
        "PUT",
        &format!("{BASE}/PolicyBundle/ghost"),
        Some(bundle("ghost", 2)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_kind_is_bad_request() {
    let app = app("aog-apiserver-k5-unknownkind").await;
    let (status, _) = send(&app, "GET", &format!("{BASE}/Bogus"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn kind_mismatch_is_bad_request() {
    let app = app("aog-apiserver-k5-kindmismatch").await;
    let body = json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "ToolGrant",
        "metadata": { "name": "x" },
        "spec": { "tool": "github" },
    });
    let (status, _) = send(&app, "POST", &format!("{BASE}/PolicyBundle"), Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
