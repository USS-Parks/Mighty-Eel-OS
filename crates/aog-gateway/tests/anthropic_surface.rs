//! G4 gate — an Anthropic-wire client completes a **message + a stream** against
//! the gateway with only a base-URL change (the anti-lock-in signal).
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (the surface authorizes the virtual key
//! against live OpenBao KV). Stands a mock Anthropic **upstream**, the gateway's
//! `surface_anthropic` router in front of it, and drives it with a raw `reqwest`
//! client sending exactly what the Anthropic SDK sends (`x-api-key` +
//! `anthropic-version`). A real Anthropic SDK pointed at the gateway is owner-gated.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::anthropic::AnthropicProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::http::header::CONTENT_TYPE;
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

const ROLE: &str = "aog-g4-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";

// Reassembles to "Hello from Anthropic".
const UPSTREAM_SSE: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":6,\"output_tokens\":1}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"from \"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Anthropic\"}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

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
        "sys/policies/acl/aog-g4-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-g4-test","token_ttl":"15m"})),
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

fn in_budget_token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: "tok_g4".to_string(),
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
        allowed_models: vec!["claude-3-5-sonnet".into()],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: Some(Budget {
            token_cap: 1_000_000,
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

// The mock Anthropic *upstream* the gateway dispatches to.
async fn upstream(Json(body): Json<Value>) -> Response {
    if body["stream"].as_bool().unwrap_or(false) {
        ([(CONTENT_TYPE, "text/event-stream")], UPSTREAM_SSE).into_response()
    } else {
        Json(json!({
            "model": "upstream-claude",
            "content": [{"type": "text", "text": "Hello from Anthropic"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 6, "output_tokens": 4}
        }))
        .into_response()
    }
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn anthropic_client_completes_message_and_stream() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP anthropic_client_completes_message_and_stream: WSF_OPENBAO_ADDR unset (G4 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-g4-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_g4"),
            json!({ "token": in_budget_token(&anchor) }),
        )
        .await
        .expect("seed key");

    let upstream_base = spawn(Router::new().route("/v1/messages", post(upstream))).await;
    let mut registry = Registry::new();
    registry.register(Arc::new(AnthropicProvider::new(upstream_base, "unused")));
    let gateway = Arc::new(Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: KV_PREFIX.to_string(),
        },
    ));
    let models = ModelMap::new().route(
        "claude-3-5-sonnet",
        Target::new("anthropic", "upstream-claude"),
    );
    let state = AppState::new(gateway, Arc::new(registry), Arc::new(models));

    let base = spawn(aog_gateway::surface_anthropic::router(state)).await;
    let http = Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()
        .unwrap();

    // --- an Anthropic client completes a message (x-api-key auth, base-URL change only) ---
    let msg = http
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "vk_g4")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(msg.status(), 200, "message status");
    let v: Value = msg.json().await.unwrap();
    assert_eq!(v["type"], "message");
    assert_eq!(v["role"], "assistant");
    assert_eq!(v["model"], "claude-3-5-sonnet");
    assert_eq!(v["content"][0]["type"], "text");
    assert_eq!(v["content"][0]["text"], "Hello from Anthropic");
    assert_eq!(v["stop_reason"], "end_turn");
    assert_eq!(v["usage"]["output_tokens"], 4);

    // --- and a stream (Anthropic event sequence, reassembling the text) ---
    let stream_resp = http
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "vk_g4")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 256,
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stream_resp.status(), 200, "stream status");
    let sse = stream_resp.text().await.unwrap();

    let mut text = String::new();
    let mut saw_start = false;
    let mut saw_stop = false;
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        let frame: Value = match serde_json::from_str(data) {
            Ok(f) => f,
            Err(_) => continue,
        };
        match frame["type"].as_str().unwrap_or_default() {
            "message_start" => saw_start = true,
            "content_block_delta" => {
                if let Some(d) = frame["delta"]["text"].as_str() {
                    text.push_str(d);
                }
            }
            "message_stop" => saw_stop = true,
            _ => {}
        }
    }
    assert!(saw_start, "stream opened with message_start");
    assert_eq!(text, "Hello from Anthropic", "stream reassembles");
    assert!(saw_stop, "stream closed with message_stop");

    // --- auth enforced: no key → 401 ---
    let unauth = http
        .post(format!("{base}/v1/messages"))
        .json(&json!({"model": "claude-3-5-sonnet", "max_tokens": 16, "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401, "unauthenticated message rejected");

    eprintln!(
        "G4 live gate PASSED against {addr} (Anthropic-wire message + stream + x-api-key auth)"
    );
}
