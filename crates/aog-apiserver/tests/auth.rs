//! The front door rejects unauth / wrong-anchor / expired / revoked /
//! over-budget requests **pre-admission**, admits a valid token, and leaves the
//! health probes open.

mod common;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::{BASE, anchor, app_anchored, authed_app, bundle, header_for, mint, mint_with, send};
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_revocation::RevocationSnapshot;

#[tokio::test]
async fn missing_token_is_unauthorized() {
    let (app, _tok) = authed_app("aog-apiserver-k6-missing").await;
    let (status, _) = send(&app, "GET", &format!("{BASE}/PolicyBundle"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_probe_is_open() {
    let (app, _tok) = authed_app("aog-apiserver-k6-health").await;
    let (status, _) = send(&app, "GET", "/healthz", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn wrong_anchor_is_unauthorized() {
    // The app trusts anchor A; the caller presents a token signed by anchor B.
    let signer_a = anchor();
    let app = app_anchored("aog-apiserver-k6-wronganchor", &signer_a, None).await;
    let signer_b = RustCryptoMlDsa87::generate("intruder").unwrap();
    let forged = header_for(&mint(&signer_b));
    let (status, _) = send(
        &app,
        "GET",
        &format!("{BASE}/PolicyBundle"),
        Some(&forged),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_token_is_unauthorized() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k6-expired", &signer, None).await;
    let expired = mint_with(&signer, |t| {
        t.expires_at = (Utc::now() - Duration::hours(1)).to_rfc3339();
    });
    let (status, _) = send(
        &app,
        "GET",
        &format!("{BASE}/PolicyBundle"),
        Some(&header_for(&expired)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn over_budget_token_is_payment_required() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k6-budget", &signer, None).await;
    let broke = mint_with(&signer, |t| {
        t.budget = Some(fabric_contracts::Budget {
            token_cap: 100,
            tokens_spent: 100,
            ..Default::default()
        });
    });
    let (status, _) = send(
        &app,
        "GET",
        &format!("{BASE}/PolicyBundle"),
        Some(&header_for(&broke)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
}

#[tokio::test]
async fn revoked_token_is_unauthorized() {
    let signer = anchor();
    let now = Utc::now();
    let mut snap = RevocationSnapshot::new(
        "snap-k6",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    );
    snap.revoked_tokens.push("tok-loom".to_owned());
    let signed_snap = fabric_revocation::sign(snap, &signer).unwrap();
    let app = app_anchored("aog-apiserver-k6-revoked", &signer, Some(signed_snap)).await;
    let (status, _) = send(
        &app,
        "GET",
        &format!("{BASE}/PolicyBundle"),
        Some(&header_for(&mint(&signer))),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn valid_token_is_admitted() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k6-valid", &signer, None).await;
    let tok = header_for(&mint(&signer));
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(&tok),
        Some(bundle("ok", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}
