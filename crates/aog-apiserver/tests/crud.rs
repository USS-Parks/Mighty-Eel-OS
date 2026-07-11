//! CRUD surface, now behind authentication: create -> get -> list ->
//! update -> delete round-trips + conflict / not-found / bad-request edges, all
//! carrying a valid trust token.

mod common;

use axum::http::StatusCode;
use common::{BASE, anchor, app_anchored, authed_app, bundle, header_for, mint, mint_with, send};

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
    // The object is authorized by a child token scoped to this action, not
    // the raw parent (attenuation).
    assert!(
        created["metadata"]["token_ref"]["token_id"]
            .as_str()
            .unwrap()
            .starts_with("child:"),
        "token_ref should be a scoped child: {created}"
    );

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
async fn create_binds_object_to_the_principal_tenant() {
    // A create body cannot smuggle a foreign metadata.tenant: the server stamps
    // the authenticated principal's tenant (tenant-loom), overwriting the spoof.
    let (app, tok) = authed_app("aog-apiserver-tenant-bind").await;
    let mut body = bundle("spoof", 1);
    body["metadata"]["tenant"] = serde_json::json!("attacker-tenant");
    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(tok.as_str()),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {created}");
    assert_eq!(
        created["metadata"]["tenant"], "tenant-loom",
        "object tenant must be the principal's, not the spoofed value: {created}"
    );
}

#[tokio::test]
async fn cross_tenant_delete_is_denied() {
    // A tenant-scoped principal may not delete another tenant's object. The
    // sharpest instance is a RevocationIntent: deleting someone else's intent
    // would reverse a live kill. tenant-loom creates the intent;
    // tenant-mallory's delete is refused and changes nothing; the owner's own
    // delete still proceeds (the denial is the tenant binding, not a blanket
    // delete refusal).
    let signer = anchor();
    let app = app_anchored("aog-apiserver-xtenant-delete", &signer, None).await;
    let owner = header_for(&mint(&signer)); // tenant-loom
    let intruder = header_for(&mint_with(&signer, |t| {
        t.token_id = "tok-mallory".to_owned();
        t.tenant_id = "tenant-mallory".to_owned();
        t.subject_hash = "hmac:mallory".to_owned();
    }));

    let intent = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "RevocationIntent",
        "metadata": { "name": "kill-tok-x" },
        "spec": {
            "target": { "target": "token", "id": "tok-x" },
            "reason": "compromised",
        },
    });
    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/RevocationIntent"),
        Some(owner.as_str()),
        Some(intent),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {created}");

    let url = format!("{BASE}/RevocationIntent/kill-tok-x");

    // The cross-tenant delete (kill-reversal) is refused with 403...
    let (status, body) = send(&app, "DELETE", &url, Some(intruder.as_str()), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a cross-tenant delete must be denied: {body}"
    );

    // ...and changed nothing: the intent is still live for its owner.
    let (status, body) = send(&app, "GET", &url, Some(owner.as_str()), None).await;
    assert_eq!(status, StatusCode::OK, "the kill intent survived: {body}");

    // The owning tenant's delete proceeds.
    let (status, body) = send(&app, "DELETE", &url, Some(owner.as_str()), None).await;
    assert!(
        status.is_success(),
        "the owner's delete must proceed, got {status}: {body}"
    );
}

#[tokio::test]
async fn cross_tenant_update_is_denied_and_tenant_is_frozen() {
    // The update verb binds tenant the same way create and delete do: a
    // tenant-scoped principal may not overwrite another tenant's object, and an
    // update body cannot reassign an object to a foreign tenant.
    let signer = anchor();
    let app = app_anchored("aog-apiserver-xtenant-update", &signer, None).await;
    let owner = header_for(&mint(&signer)); // tenant-loom
    let intruder = header_for(&mint_with(&signer, |t| {
        t.token_id = "tok-mallory".to_owned();
        t.tenant_id = "tenant-mallory".to_owned();
        t.subject_hash = "hmac:mallory".to_owned();
    }));

    // tenant-loom owns the bundle.
    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(owner.as_str()),
        Some(bundle("xtenant", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {created}");

    let url = format!("{BASE}/PolicyBundle/xtenant");

    // A cross-tenant update is refused with 403...
    let (status, body) = send(
        &app,
        "PUT",
        &url,
        Some(intruder.as_str()),
        Some(bundle("xtenant", 99)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a cross-tenant update must be denied: {body}"
    );

    // ...and changed nothing: the owner still sees version 1.
    let (status, got) = send(&app, "GET", &url, Some(owner.as_str()), None).await;
    assert_eq!(status, StatusCode::OK, "the object survived: {got}");
    assert_eq!(
        got["spec"]["version"], 1,
        "the spec must be untouched: {got}"
    );

    // The owner's own update proceeds, but a body smuggling a foreign tenant is
    // neutralized: the object keeps tenant-loom.
    let mut spoof = bundle("xtenant", 2);
    spoof["metadata"]["tenant"] = serde_json::json!("tenant-mallory");
    let (status, updated) = send(&app, "PUT", &url, Some(owner.as_str()), Some(spoof)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the owner's update must proceed: {updated}"
    );
    assert_eq!(updated["spec"]["version"], 2);
    assert_eq!(
        updated["metadata"]["tenant"], "tenant-loom",
        "an update body must not reassign the object's tenant: {updated}"
    );
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
