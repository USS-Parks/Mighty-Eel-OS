//! K5 gate — "no write reaches aog-store bypassing admission" (the type half is
//! the private node handle + read-only reader; see the `admission` module docs).
//! Behavioral proof: a request that fails an admission stage persists **nothing**,
//! and every admitted object bears the stamps only the admission stages apply.

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{BASE, authed_app, send};

async fn list_len(app: &Router, tok: &str, kind: &str) -> usize {
    let (status, listed) = send(app, "GET", &format!("{BASE}/{kind}"), Some(tok), None).await;
    assert_eq!(status, StatusCode::OK);
    listed["items"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn admission_reject_persists_nothing() {
    let (app, tok) = authed_app("aog-apiserver-k5-rejectpersist").await;
    let t = Some(tok.as_str());

    // Structurally invalid: a PolicyBundle version 0 fails estate validation.
    let bad_spec = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "reject-me" },
        "spec": { "version": 0 },
    });
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bad_spec),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Invalid name: illegal characters fail metadata validation.
    let bad_name = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "Reject_Me" },
        "spec": { "version": 1 },
    });
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bad_name),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Neither rejected write reached the store.
    assert_eq!(
        list_len(&app, &tok, "PolicyBundle").await,
        0,
        "a rejected admission must not persist anything"
    );
}

#[tokio::test]
async fn admitted_object_bears_admission_stamps() {
    let (app, tok) = authed_app("aog-apiserver-k5-stamps").await;
    let t = Some(tok.as_str());
    let good = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "stamped" },
        "spec": { "version": 1 },
    });
    let (status, created) =
        send(&app, "POST", &format!("{BASE}/PolicyBundle"), t, Some(good)).await;
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
    let (status, got) = send(
        &app,
        "GET",
        &format!("{BASE}/PolicyBundle/stamped"),
        t,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["metadata"]["uid"], created["metadata"]["uid"]);
    assert_eq!(got["metadata"]["resource_version"], 1);
}
