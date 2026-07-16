//! G7 gate — **cost-per-task aggregation across a multi-call chain** + the
//! **receipt chain verifies** — and the stream-budget gate — **a streamed call accrues
//! spend, and a budgeted key crossing its cap on the stream path is refused**.
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. Seeds an in-budget virtual key, stands a mock
//! upstream that reports fixed usage, sends a multi-call chain tagged with an
//! `x-aog-workflow` task id, then reads `/v1/usage` and asserts the aggregated
//! cost + a verified receipt chain. The stream-budget leg seeds a tight-cap key, streams one
//! SSE call to completion, and asserts the next call is refused `402`.
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

const ROLE: &str = "aog-g7-test";
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
        "sys/policies/acl/aog-g7-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-g7-test","token_ttl":"15m"})),
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

fn in_budget_token(signer: &RustCryptoMlDsa87, token_id: &str, token_cap: u64) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: token_id.to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::minutes(15)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.03".to_string(),
        tenant_id: "tenant-a".to_string(),
        subject_id: None,
        subject_hash: "hmac-sha256:demo".to_string(),
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
            token_cap,
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

// Fixed usage: 1000 in + 500 out. At the baseline gpt-4o-mini price
// (15/1k in, 60/1k out) that is 15 + 30 = 45 cents per call.
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

async fn chat(http: &Client, base: &str, workflow: &str) {
    let r = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_g7")
        .header("x-aog-workflow", workflow)
        .json(
            &json!({ "model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hello"}] }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "chat call in the {workflow} chain");
}

#[tokio::test]
async fn cost_per_task_aggregates_and_chain_verifies() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP cost_per_task_aggregates_and_chain_verifies: WSF_OPENBAO_ADDR unset (G7 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-g7-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_g7"),
            json!({ "token": in_budget_token(&anchor, "tok_g7", 100_000_000) }),
        )
        .await
        .expect("seed key");

    let upstream_base = spawn(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::new(
        "openai",
        upstream_base,
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

    // A multi-call chain: 3 calls under task-alpha, 1 under task-beta.
    chat(&http, &base, "task-alpha").await;
    chat(&http, &base, "task-alpha").await;
    chat(&http, &base, "task-alpha").await;
    chat(&http, &base, "task-beta").await;

    // aog-meter aggregation + a live chain verify.
    let usage: Value = http
        .get(format!("{base}/v1/usage"))
        .bearer_auth("vk_g7")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(usage["chain_verified"], true, "receipt chain verifies");
    assert!(
        usage["chain_head"].as_str().is_some_and(|h| h.len() == 64),
        "chain head is a hex digest"
    );

    let aggs = usage["aggregates"].as_array().expect("aggregates array");
    let alpha = aggs
        .iter()
        .find(|a| a["workflow_id"] == "task-alpha")
        .expect("task-alpha group present");
    assert_eq!(alpha["calls"], 3, "3 calls in the task-alpha chain");
    assert_eq!(alpha["spend_cents"], 135, "cost-per-task = 3 × 45 cents");
    assert_eq!(alpha["provider"], "openai");
    assert_eq!(alpha["model"], "gpt-4o-mini");
    assert_eq!(alpha["input_tokens"], 3000);
    assert_eq!(alpha["output_tokens"], 1500);

    let beta = aggs
        .iter()
        .find(|a| a["workflow_id"] == "task-beta")
        .expect("task-beta group");
    assert_eq!(beta["calls"], 1);
    assert_eq!(beta["spend_cents"], 45);

    eprintln!(
        "G7 live gate PASSED against {addr} (cost-per-task aggregation across a multi-call chain; receipt chain verifies)"
    );
}

/// SSE upstream for the stream-budget leg: two content deltas, a terminal usage frame
/// (1000 in + 500 out — the shape `stream_options.include_usage` yields), then
/// `[DONE]`.
async fn upstream_sse(Json(_body): Json<Value>) -> Response {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"str\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"eam\"}}]}\n\n",
        "data: {\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":500},\"choices\":[]}\n\n",
        "data: [DONE]\n\n",
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
        .into_response()
}

#[tokio::test]
async fn streamed_call_accrues_spend_and_cap_refuses_next_call() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP streamed_call_accrues_spend_and_cap_refuses_next_call: WSF_OPENBAO_ADDR unset (stream-budget live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-r8-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    // Cap below one streamed call's usage (1000 + 500): the first stream passes
    // pre-flight (nothing spent yet), settles 1500 at stream end, and every call
    // after that must be refused 402 on the folded spend.
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_r8s"),
            json!({ "token": in_budget_token(&anchor, "tok_r8s", 1_400) }),
        )
        .await
        .expect("seed key");

    let upstream_base =
        spawn(Router::new().route("/v1/chat/completions", post(upstream_sse))).await;
    let mut registry = Registry::new();
    registry.register(Arc::new(OpenAiProvider::new(
        "openai",
        upstream_base,
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

    // First call: admitted (budget untouched) and streamed to completion.
    let r = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_r8s")
        .json(&json!({
            "model": "gpt-4o-mini",
            "stream": true,
            "messages": [{"role": "user", "content": "stream me"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "first streamed call is admitted");
    let body = r.text().await.unwrap();
    assert!(body.contains("str"), "deltas streamed through");
    assert!(body.contains("[DONE]"), "stream ran to completion");

    // Settlement fires when the server drops the SSE generator; give that drop
    // a short bounded window, then require the refusal. (An extra 200 during
    // the window only accrues more spend — convergence is monotone.)
    let mut refused = false;
    for _ in 0..50 {
        let r2 = http
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth("vk_r8s")
            .json(&json!({
                "model": "gpt-4o-mini",
                "messages": [{"role": "user", "content": "again"}]
            }))
            .send()
            .await
            .unwrap();
        if r2.status() == 402 {
            refused = true;
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(100)).await;
    }
    assert!(
        refused,
        "a budgeted key crossing its cap on the STREAM path is refused 402 pre-flight (the stream-budget gate)"
    );
    eprintln!(
        "stream-budget live gate PASSED against {addr} (streamed call accrued spend; cap crossed → next call 402)"
    );
}
