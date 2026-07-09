//! A stored object at an older schema version is served at the hub version,
//! converted transparently on read; the default (identity) registry serves stored
//! objects unchanged.

mod common;

use aog_apiserver::convert::ConversionRegistry;
use aog_estate::Kind;
use axum::http::StatusCode;
use common::{BASE, authed_app, authed_app_with_conversions, bundle, send};
use serde_json::json;

#[tokio::test]
async fn stored_v1_is_served_at_the_v2_hub() {
    // The estate validates + stores v1; the server's hub is v2 with a v1->v2
    // converter, so a stored v1 object is upgraded on read.
    let registry = ConversionRegistry::new("aog.islandmountain.io/v2").with_converter(
        Kind::PolicyBundle,
        "aog.islandmountain.io/v1",
        |mut v| {
            v["api_version"] = json!("aog.islandmountain.io/v2");
            v["spec"]["tier"] = json!("standard");
            v
        },
    );
    let (app, _state, tok) = authed_app_with_conversions("aog-apiserver-k10", registry).await;
    let t = Some(tok.as_str());

    // Create a v1 PolicyBundle (the only schema the estate knows).
    let (s, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bundle("cfg", 1)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // Read it back: served as v2 (converted on read), original field preserved.
    let (s, got) = send(&app, "GET", &format!("{BASE}/PolicyBundle/cfg"), t, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["api_version"], "aog.islandmountain.io/v2");
    assert_eq!(got["spec"]["tier"], "standard");
    assert_eq!(got["spec"]["version"], 1, "the original field is preserved");

    // List converts each entry too.
    let (s, listed) = send(&app, "GET", &format!("{BASE}/PolicyBundle"), t, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        listed["items"][0]["api_version"],
        "aog.islandmountain.io/v2"
    );
}

#[tokio::test]
async fn default_registry_serves_stored_version_unchanged() {
    let (app, tok) = authed_app("aog-apiserver-k10-identity").await;
    let t = Some(tok.as_str());
    let (s, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bundle("cfg", 1)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, got) = send(&app, "GET", &format!("{BASE}/PolicyBundle/cfg"), t, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        got["api_version"], "aog.islandmountain.io/v1",
        "identity registry performs no conversion"
    );
}
