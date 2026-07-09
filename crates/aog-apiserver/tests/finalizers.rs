//! Two-phase delete + finalizer semantics.
//!
//! Delete with finalizers = soft delete (deletion_timestamp stamped, object
//! kept, teardown runs); removing the last finalizer completes the delete.
//! While terminating: spec frozen, finalizers only shrink, the deletion
//! timestamp cannot be cleared. Repeat deletes are idempotent no-ops that
//! write no receipt (receipts stay 1:1 with mutations).

mod common;

use serde_json::{Value, json};

use common::{BASE, authed_app, authed_app_state, bundle, send};

/// A PolicyBundle body carrying finalizers.
fn bundle_with_finalizers(name: &str, version: u32, finalizers: &[&str]) -> Value {
    json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": name, "finalizers": finalizers },
        "spec": { "version": version },
    })
}

#[tokio::test]
async fn delete_without_finalizers_removes_immediately() {
    let (app, token) = authed_app("loom-r2-hard-delete").await;
    let uri = format!("{BASE}/PolicyBundle");
    send(&app, "POST", &uri, Some(&token), Some(bundle("plain", 1))).await;

    let one = format!("{BASE}/PolicyBundle/plain");
    let (status, _) = send(&app, "DELETE", &one, Some(&token), None).await;
    assert_eq!(status, 204);
    let (status, _) = send(&app, "GET", &one, Some(&token), None).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn delete_with_finalizers_soft_deletes_and_repeat_is_receiptless() {
    let (app, state, token) = authed_app_state("loom-r2-soft-delete").await;
    let uri = format!("{BASE}/PolicyBundle");
    send(
        &app,
        "POST",
        &uri,
        Some(&token),
        Some(bundle_with_finalizers("guarded", 1, &["loom.aog/teardown"])),
    )
    .await;

    // First delete: soft — the terminating object comes back, still readable.
    let one = format!("{BASE}/PolicyBundle/guarded");
    let (status, body) = send(&app, "DELETE", &one, Some(&token), None).await;
    assert_eq!(status, 200);
    assert!(
        body["metadata"]["deletion_timestamp"].is_string(),
        "soft delete stamps deletion_timestamp"
    );
    let (status, body) = send(&app, "GET", &one, Some(&token), None).await;
    assert_eq!(status, 200);
    assert!(body["metadata"]["deletion_timestamp"].is_string());

    // Repeat delete: idempotent no-op — same terminating object, no new receipt.
    let receipts_before = state.receipts_len();
    let (status, body) = send(&app, "DELETE", &one, Some(&token), None).await;
    assert_eq!(status, 200);
    assert!(body["metadata"]["deletion_timestamp"].is_string());
    assert_eq!(
        state.receipts_len(),
        receipts_before,
        "an admitted no-op mutates nothing and writes no receipt"
    );
}

#[tokio::test]
async fn terminating_object_spec_frozen_and_timestamp_sticky() {
    let (app, token) = authed_app("loom-r2-frozen").await;
    let uri = format!("{BASE}/PolicyBundle");
    send(
        &app,
        "POST",
        &uri,
        Some(&token),
        Some(bundle_with_finalizers("frozen", 1, &["loom.aog/teardown"])),
    )
    .await;
    let one = format!("{BASE}/PolicyBundle/frozen");
    let (status, _) = send(&app, "DELETE", &one, Some(&token), None).await;
    assert_eq!(status, 200);
    let (_, current) = send(&app, "GET", &one, Some(&token), None).await;

    // Spec change while terminating → refused.
    let mut spec_bump = current.clone();
    spec_bump["spec"]["version"] = json!(2);
    let (status, body) = send(&app, "PUT", &one, Some(&token), Some(spec_bump)).await;
    assert_eq!(status, 409, "spec is frozen while terminating: {body}");

    // Growing the finalizer set while terminating → refused.
    let mut grow = current.clone();
    grow["metadata"]["finalizers"] = json!(["loom.aog/teardown", "loom.aog/extra"]);
    let (status, _) = send(&app, "PUT", &one, Some(&token), Some(grow)).await;
    assert_eq!(status, 409, "finalizers may only shrink while terminating");

    // Omitting deletion_timestamp cannot resurrect the object.
    let mut resurrect = current.clone();
    resurrect["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("deletion_timestamp");
    let (status, body) = send(&app, "PUT", &one, Some(&token), Some(resurrect)).await;
    assert_eq!(status, 200);
    assert!(
        body["metadata"]["deletion_timestamp"].is_string(),
        "the deletion timestamp is carried forward, never cleared"
    );
}

#[tokio::test]
async fn removing_last_finalizer_completes_the_delete() {
    let (app, state, token) = authed_app_state("loom-r2-finalize").await;
    let uri = format!("{BASE}/PolicyBundle");
    send(
        &app,
        "POST",
        &uri,
        Some(&token),
        Some(bundle_with_finalizers("held", 1, &["loom.aog/teardown"])),
    )
    .await;
    let one = format!("{BASE}/PolicyBundle/held");
    send(&app, "DELETE", &one, Some(&token), None).await;
    let (_, current) = send(&app, "GET", &one, Some(&token), None).await;

    // Teardown done: the finalizing controller strips its finalizer.
    let receipts_before = state.receipts_len();
    let mut finalize = current.clone();
    finalize["metadata"]["finalizers"] = json!([]);
    let (status, body) = send(&app, "PUT", &one, Some(&token), Some(finalize)).await;
    assert_eq!(status, 200);
    assert_eq!(body["finalized"], json!(true));
    assert_eq!(
        state.receipts_len(),
        receipts_before + 1,
        "the finalizing update commits (and receipts) the hard delete"
    );

    let (status, _) = send(&app, "GET", &one, Some(&token), None).await;
    assert_eq!(status, 404, "the object is gone once finalized");
}

#[tokio::test]
async fn stale_resource_version_is_refused() {
    let (app, token) = authed_app("loom-r2-stale").await;
    let uri = format!("{BASE}/PolicyBundle");
    let (_, created) = send(&app, "POST", &uri, Some(&token), Some(bundle("cas", 1))).await;
    let first_rv = created["metadata"]["resource_version"].clone();

    // A legacy update (no resource_version asserted) still lands.
    let one = format!("{BASE}/PolicyBundle/cas");
    let (status, _) = send(&app, "PUT", &one, Some(&token), Some(bundle("cas", 2))).await;
    assert_eq!(status, 200);

    // An update asserting the stale revision is refused, not silently applied.
    let mut stale = bundle("cas", 3);
    stale["metadata"]["resource_version"] = first_rv;
    let (status, body) = send(&app, "PUT", &one, Some(&token), Some(stale)).await;
    assert_eq!(status, 409, "stale write refused: {body}");
}
