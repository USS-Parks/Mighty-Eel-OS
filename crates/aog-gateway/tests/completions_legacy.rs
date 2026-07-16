//! The legacy `/v1/completions` endpoint runs the FULL governed pipeline —
//! auth, classify/route, policy, tokenized egress, meter/receipt, budget —
//! never a bare provider call.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (auth resolves the virtual key against
//! live OpenBao KV). Two legs:
//!
//! * a benign prompt: refused 401 without a key; 200 with one, in the legacy
//!   wire shape, carrying the route/policy governance headers; the call is
//!   metered (`/v1/usage` aggregates it and the receipt chain verifies);
//! * a PHI-shaped prompt: the legacy endpoint and `/v1/chat/completions`
//!   return the SAME policy outcome (status + policy headers) — the exact
//!   parity the endpoint's governance comment promises — and no upstream
//!   request ever carries the raw span.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::extract::State;
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

const ROLE: &str = "aog-af10-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const RAW_SSN: &str = "123-45-6789";

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
        "sys/policies/acl/aog-af10-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-af10-test","token_ttl":"15m"})),
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

fn in_budget_token(signer: &RustCryptoMlDsa87, token_id: &str) -> TrustToken {
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

/// Everything the mock upstream was ever sent, for the raw-span assertion.
type Captured = Arc<Mutex<Vec<Value>>>;

/// Fixed usage: 1000 in + 500 out (45 cents at the baseline gpt-4o-mini price).
async fn upstream(State(captured): State<Captured>, Json(body): Json<Value>) -> Response {
    captured.lock().unwrap().push(body);
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

fn policy_headers(r: &reqwest::Response) -> (u16, Option<String>, Option<String>) {
    let h = |name: &str| {
        r.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    (
        r.status().as_u16(),
        h("x-aog-policy-mode"),
        h("x-aog-policy-blocked"),
    )
}

#[tokio::test]
async fn legacy_completions_runs_the_full_governed_pipeline() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP legacy_completions_runs_the_full_governed_pipeline: WSF_OPENBAO_ADDR unset (live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-af10-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_af10"),
            json!({ "token": in_budget_token(&anchor, "tok_af10") }),
        )
        .await
        .expect("seed key");

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let upstream_base = spawn(
        Router::new()
            .route("/v1/chat/completions", post(upstream))
            .with_state(Arc::clone(&captured)),
    )
    .await;
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

    // Unauthenticated legacy call -> refused (never a bare provider call).
    let r = http
        .post(format!("{base}/v1/completions"))
        .json(&json!({ "model": "gpt-4o-mini", "prompt": "hello" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "the legacy endpoint authenticates first"
    );

    // Authenticated benign prompt -> the legacy wire shape, governance-tagged.
    let r = http
        .post(format!("{base}/v1/completions"))
        .bearer_auth("vk_af10")
        .json(&json!({ "model": "gpt-4o-mini", "prompt": "hello" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "benign legacy completion is admitted");
    assert!(
        r.headers().contains_key("x-aog-route"),
        "the call was classified/routed (x-aog-route)"
    );
    assert!(
        r.headers().contains_key("x-aog-policy-mode"),
        "the call passed the policy gate (x-aog-policy-mode)"
    );
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["object"], "text_completion", "legacy wire shape");
    assert_eq!(body["choices"][0]["text"], "ok");
    assert_eq!(body["usage"]["total_tokens"], 1500);

    // The call was metered: /v1/usage aggregates it and the chain verifies.
    let usage: Value = http
        .get(format!("{base}/v1/usage"))
        .bearer_auth("vk_af10")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(usage["chain_verified"], true, "receipt chain verifies");
    let aggs = usage["aggregates"].as_array().expect("aggregates array");
    let total_calls: u64 = aggs.iter().filter_map(|a| a["calls"].as_u64()).sum();
    let total_spend: u64 = aggs.iter().filter_map(|a| a["spend_cents"].as_u64()).sum();
    assert_eq!(total_calls, 1, "the legacy call was metered: {usage}");
    assert_eq!(total_spend, 45, "the legacy call was priced: {usage}");

    // A PHI-shaped prompt gets the SAME policy outcome on the legacy path as
    // on chat — the parity that makes /v1/completions governed rather than a
    // side door. Whichever way the gate falls (block, or admit tokenized),
    // the two surfaces must agree.
    let phi = format!("Patient SSN {RAW_SSN} — summarize the chart");
    let legacy = http
        .post(format!("{base}/v1/completions"))
        .bearer_auth("vk_af10")
        .json(&json!({ "model": "gpt-4o-mini", "prompt": phi }))
        .send()
        .await
        .unwrap();
    let legacy_outcome = policy_headers(&legacy);
    let chat = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_af10")
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": phi}]
        }))
        .send()
        .await
        .unwrap();
    let chat_outcome = policy_headers(&chat);
    assert_eq!(
        legacy_outcome, chat_outcome,
        "legacy and chat must reach the same policy outcome for the same prompt"
    );

    // Whatever was admitted upstream, the raw span never egressed.
    let seen = captured.lock().unwrap();
    for req in seen.iter() {
        let s = req.to_string();
        assert!(
            !s.contains(RAW_SSN),
            "the upstream must never see the raw SSN: {s}"
        );
    }

    eprintln!(
        "legacy-completions live gate PASSED against {addr} (authenticated, \
         routed, policy-gated in parity with chat, metered, chain-verified; no raw span egressed)"
    );
}
