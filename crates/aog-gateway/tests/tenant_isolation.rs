//! `/v1/usage` and `/v1/roi` are computed only over the CALLER'S tenant —
//! seeding a second tenant proves the scoping (one tenant can never read
//! another's provider/model/spend estate).
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. Seeds two virtual keys under different
//! tenants, drives 2 calls as tenant-a and 1 as tenant-b through one shared
//! gateway, then asserts each key's usage aggregates and ROI report carry
//! exactly its own tenant's calls and spend.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Route, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "aog-af17-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}
fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

async fn bao(
    c: &Client,
    addr: &str,
    tok: &str,
    m: Method,
    path: &str,
    body: Option<Value>,
) -> String {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(m, &url).header("X-Vault-Token", tok);
    if let Some(b) = body {
        rb = rb
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    rb.send()
        .await
        .expect("openbao req")
        .text()
        .await
        .unwrap_or_default()
}

async fn provision(c: &Client, addr: &str, tok: &str) -> (String, String) {
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type":"approle"})),
    )
    .await;
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/mounts/kv",
        Some(json!({"type":"kv","options":{"version":"2"}})),
    )
    .await;
    let policy = "path \"kv/data/aog/*\" { capabilities=[\"create\",\"update\",\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/aog-af17-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-af17-test","token_ttl":"15m"})),
    )
    .await;
    let rid: Value = serde_json::from_str(
        &bao(
            c,
            addr,
            tok,
            Method::GET,
            &format!("auth/approle/role/{ROLE}/role-id"),
            None,
        )
        .await,
    )
    .expect("role-id json");
    let role_id = rid["data"]["role_id"]
        .as_str()
        .expect("role_id")
        .to_string();
    let sid: Value = serde_json::from_str(
        &bao(
            c,
            addr,
            tok,
            Method::POST,
            &format!("auth/approle/role/{ROLE}/secret-id"),
            Some(json!({})),
        )
        .await,
    )
    .expect("secret-id json");
    let secret_id = sid["data"]["secret_id"]
        .as_str()
        .expect("secret_id")
        .to_string();
    (role_id, secret_id)
}

fn tenant_token(signer: &RustCryptoMlDsa87, token_id: &str, tenant: &str) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: token_id.to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::minutes(15)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.03".to_string(),
        tenant_id: tenant.to_string(),
        subject_id: None,
        subject_hash: format!("hmac-sha256:{tenant}"),
        service_identity: None,
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![Route::CloudAllowed],
        allowed_models: vec!["gpt-4o-mini".into()],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: Some(Budget {
            token_cap: 100_000_000,
            tokens_spent: 0,
            ..Default::default()
        }),
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(t, signer).unwrap()
}

fn key_path(virtual_key: &str) -> String {
    format!(
        "{KV_PREFIX}/{}",
        hex::encode(Sha256::digest(virtual_key.as_bytes()))
    )
}

/// Fixed usage: 1000 in + 500 out (45 cents at the baseline gpt-4o-mini price).
async fn upstream(Json(_body): Json<Value>) -> Response {
    Json(json!({
        "model": "upstream-x",
        "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 500}
    }))
    .into_response()
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn chat(http: &Client, base: &str, vk: &str) {
    let r = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(vk)
        .json(
            &json!({ "model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hello"}] }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "chat call under {vk}");
}

async fn get_json(http: &Client, base: &str, path: &str, vk: &str) -> Value {
    http.get(format!("{base}{path}"))
        .bearer_auth(vk)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn usage_and_roi_are_scoped_to_the_calling_tenant() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP usage_and_roi_are_scoped_to_the_calling_tenant: WSF_OPENBAO_ADDR unset (live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-af17-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_af17_a"),
            json!({ "token": tenant_token(&anchor, "tok_af17_a", "tenant-a") }),
        )
        .await
        .expect("seed tenant-a key");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_af17_b"),
            json!({ "token": tenant_token(&anchor, "tok_af17_b", "tenant-b") }),
        )
        .await
        .expect("seed tenant-b key");

    let upstream_base = spawn(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::new(
        "openai",
        aog_gateway::posture::ApprovedProviderEndpoint::loopback_fixture(&upstream_base).unwrap(),
        "unused",
    )));
    let gateway = Arc::new(Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: KV_PREFIX.to_string(),
        },
    ));
    let models = ModelMap::new().route("gpt-4o-mini", Target::new("openai", "upstream-x"));
    let state = AppState::new(gateway, Arc::new(registry), Arc::new(models));

    let base = spawn(aog_gateway::surface_openai::router(state)).await;
    let http = Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()
        .unwrap();

    // One shared gateway: 2 calls as tenant-a, 1 as tenant-b.
    chat(&http, &base, "vk_af17_a").await;
    chat(&http, &base, "vk_af17_a").await;
    chat(&http, &base, "vk_af17_b").await;

    // tenant-a's view: exactly its own 2 calls / 90 cents, nothing of b's.
    let usage_a = get_json(&http, &base, "/v1/usage", "vk_af17_a").await;
    let aggs_a = usage_a["aggregates"].as_array().expect("aggregates array");
    assert!(
        aggs_a.iter().all(|a| a["tenant_id"] == "tenant-a"),
        "tenant-a must see only tenant-a aggregates: {usage_a}"
    );
    let calls_a: u64 = aggs_a.iter().filter_map(|a| a["calls"].as_u64()).sum();
    let spend_a: u64 = aggs_a
        .iter()
        .filter_map(|a| a["spend_cents"].as_u64())
        .sum();
    assert_eq!(calls_a, 2, "tenant-a sees exactly its two calls: {usage_a}");
    assert_eq!(
        spend_a, 90,
        "tenant-a sees exactly its own spend: {usage_a}"
    );

    // tenant-b's view: exactly its own 1 call / 45 cents, nothing of a's.
    let usage_b = get_json(&http, &base, "/v1/usage", "vk_af17_b").await;
    let aggs_b = usage_b["aggregates"].as_array().expect("aggregates array");
    assert!(
        aggs_b.iter().all(|a| a["tenant_id"] == "tenant-b"),
        "tenant-b must see only tenant-b aggregates: {usage_b}"
    );
    let calls_b: u64 = aggs_b.iter().filter_map(|a| a["calls"].as_u64()).sum();
    let spend_b: u64 = aggs_b
        .iter()
        .filter_map(|a| a["spend_cents"].as_u64())
        .sum();
    assert_eq!(calls_b, 1, "tenant-b sees exactly its one call: {usage_b}");
    assert_eq!(
        spend_b, 45,
        "tenant-b sees exactly its own spend: {usage_b}"
    );

    // ROI is computed over the caller's own tenant spend only.
    let roi_a = get_json(&http, &base, "/v1/roi", "vk_af17_a").await;
    let roi_b = get_json(&http, &base, "/v1/roi", "vk_af17_b").await;
    assert_eq!(
        roi_a["cloud_spend_cents"], 90,
        "tenant-a ROI covers only tenant-a spend: {roi_a}"
    );
    assert_eq!(
        roi_b["cloud_spend_cents"], 45,
        "tenant-b ROI covers only tenant-b spend: {roi_b}"
    );

    eprintln!(
        "tenant-isolation live gate PASSED against {addr} (usage + ROI scoped per tenant: 2×45 vs 1×45, no cross-tenant leakage)"
    );
}
