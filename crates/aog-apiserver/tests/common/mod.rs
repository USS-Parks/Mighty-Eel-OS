//! Shared harness for the aog-apiserver integration tests: an authenticated app
//! (K6 front door) and helpers to mint ML-DSA trust tokens and send JSON
//! requests in-process (`tower::ServiceExt::oneshot`, no socket bound).
#![allow(dead_code)]

use std::path::PathBuf;

use aog_apiserver::auth::{Authenticator, TOKEN_HEADER};
use aog_apiserver::convert::ConversionRegistry;
use aog_apiserver::seal::Sealer;
use aog_apiserver::{AppState, router};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Duration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_revocation::RevocationSnapshot;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const BASE: &str = "/apis/aog.islandmountain.io/v1";

pub fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fresh trust-anchor signer.
pub fn anchor() -> RustCryptoMlDsa87 {
    RustCryptoMlDsa87::generate("loom-test-anchor").unwrap()
}

/// Mint a signed token under `signer`, customised by `f` before signing.
pub fn mint_with(signer: &RustCryptoMlDsa87, f: impl FnOnce(&mut TrustToken)) -> TrustToken {
    let now = Utc::now();
    let mut token = TrustToken {
        token_id: "tok-loom".to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: "tenant-loom".to_owned(),
        subject_id: None,
        subject_hash: "hmac:loom".to_owned(),
        service_identity: Some("aogctl".to_owned()),
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    f(&mut token);
    fabric_token::issue(token, signer).unwrap()
}

/// A plain valid token under `signer`.
pub fn mint(signer: &RustCryptoMlDsa87) -> TrustToken {
    mint_with(signer, |_| {})
}

/// The `x-wsf-token` header value for a token.
pub fn header_for(token: &TrustToken) -> String {
    BASE64.encode(serde_json::to_vec(token).unwrap())
}

/// A fresh app anchored on `signer`'s key, with an optional revocation snapshot.
pub async fn app_anchored(
    dir_name: &str,
    signer: &RustCryptoMlDsa87,
    revocation: Option<RevocationSnapshot>,
) -> Router {
    let mut auth = Authenticator::new(signer.public_key().to_vec());
    if let Some(snap) = revocation {
        auth = auth.with_revocation(snap).unwrap();
    }
    let state = AppState::bootstrap(1, fresh_dir(dir_name), auth, Sealer::generate().unwrap())
        .await
        .unwrap();
    router(state)
}

/// The common case: a fresh authenticated app + a valid token header for it.
pub async fn authed_app(dir_name: &str) -> (Router, String) {
    let signer = anchor();
    let app = app_anchored(dir_name, &signer, None).await;
    (app, header_for(&mint(&signer)))
}

/// Like [`authed_app`] but also returns a clone of the [`AppState`] so a test can
/// inspect the receipt ledger (K9).
pub async fn authed_app_state(dir_name: &str) -> (Router, AppState, String) {
    let signer = anchor();
    let auth = Authenticator::new(signer.public_key().to_vec());
    let state = AppState::bootstrap(1, fresh_dir(dir_name), auth, Sealer::generate().unwrap())
        .await
        .unwrap();
    let header = header_for(&mint(&signer));
    (router(state.clone()), state, header)
}

/// An authenticated app + state configured with a K10 conversion registry.
pub async fn authed_app_with_conversions(
    dir_name: &str,
    conversions: ConversionRegistry,
) -> (Router, AppState, String) {
    let signer = anchor();
    let auth = Authenticator::new(signer.public_key().to_vec());
    let state = AppState::bootstrap(1, fresh_dir(dir_name), auth, Sealer::generate().unwrap())
        .await
        .unwrap()
        .with_conversions(conversions);
    let header = header_for(&mint(&signer));
    (router(state.clone()), state, header)
}

/// Send a request, optionally carrying the `x-wsf-token` header.
pub async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(TOKEN_HEADER, t);
    }
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

/// A minimal valid `PolicyBundle` body.
pub fn bundle(name: &str, version: u32) -> Value {
    json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": name },
        "spec": { "version": version },
    })
}
