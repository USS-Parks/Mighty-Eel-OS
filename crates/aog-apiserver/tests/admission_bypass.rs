//! K5 gate — "no write reaches `aog-store` bypassing admission."
//!
//! The type half of the gate is structural (see the `admission` module docs):
//! the only writable store handle in the crate is private to `Admission`, and
//! `admit` is its only writer; a handler holds `Admission` + a read-only
//! `StoreReader` and cannot reach the node otherwise. These tests prove the
//! behavioral consequence — a request that fails an admission stage persists
//! **nothing**, and every persisted object carries the stamps that only the
//! admission `mutate` stage applies (so a write that skipped admission could not
//! have produced it).

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
    router(AppState::bootstrap(1, fresh_dir(dir_name)).await.unwrap())
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

async fn list_len(app: &Router, kind: &str) -> usize {
    let (status, listed) = send(app, "GET", &format!("{BASE}/{kind}"), None).await;
    assert_eq!(status, StatusCode::OK);
    listed["items"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn admission_reject_persists_nothing() {
    let app = app("aog-apiserver-k5-rejectpersist").await;

    // Structurally invalid: a PolicyBundle version 0 fails estate validation.
    let bad_spec = json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "reject-me" },
        "spec": { "version": 0 },
    });
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(bad_spec),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Invalid name: illegal characters fail metadata validation.
    let bad_name = json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "Reject_Me" },
        "spec": { "version": 1 },
    });
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(bad_name),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Neither rejected write reached the store.
    assert_eq!(
        list_len(&app, "PolicyBundle").await,
        0,
        "a rejected admission must not persist anything"
    );
}

#[tokio::test]
async fn admitted_object_bears_admission_stamps() {
    let app = app("aog-apiserver-k5-stamps").await;
    let good = json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "stamped" },
        "spec": { "version": 1 },
    });
    let (status, created) = send(&app, "POST", &format!("{BASE}/PolicyBundle"), Some(good)).await;
    assert_eq!(status, StatusCode::CREATED);

    // uid / generation / created_at / resource_version are applied by the
    // admission mutate + commit stages — a bypassing write could not set them.
    assert!(
        created["metadata"]["uid"]
            .as_str()
            .is_some_and(|u| !u.is_empty())
    );
    assert_eq!(created["metadata"]["generation"], 1);
    assert!(created["metadata"]["created_at"].as_str().is_some());
    assert_eq!(created["metadata"]["resource_version"], 1);

    // The persisted copy carries the same identity on read-back.
    let (status, got) = send(&app, "GET", &format!("{BASE}/PolicyBundle/stamped"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["metadata"]["uid"], created["metadata"]["uid"]);
    assert_eq!(got["metadata"]["resource_version"], 1);
}
