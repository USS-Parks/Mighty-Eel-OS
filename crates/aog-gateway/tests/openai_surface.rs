//! G3 gate — an OpenAI-wire client completes a **chat + a stream** against the
//! gateway with only a base-URL change.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (the surface authorizes the virtual key
//! against live OpenBao KV — G1). Stands up a mock OpenAI **upstream** (the model
//! backend), the gateway's `surface_openai` router in front of it, and drives it
//! with a raw `reqwest` client sending exactly what an off-the-shelf OpenAI SDK
//! sends. Real-SDK (`openai-python` / `async-openai`) pointed at the gateway is
//! owner-gated; the wire contract those SDKs depend on is what this asserts.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::provider::Registry;
use aog_gateway::provider::openai::OpenAiProvider;
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

const ROLE: &str = "aog-g3-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";

// Reassembles to "Hello from the gateway".
const UPSTREAM_SSE: &str = "\
data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"from \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"the \"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"gateway\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":4}}\n\n\
data: [DONE]\n\n";

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
        "sys/policies/acl/aog-g3-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-g3-test","token_ttl":"15m"})),
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
        token_id: "tok_g3".to_string(),
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

// The mock OpenAI *upstream* the gateway dispatches to.
async fn upstream(Json(body): Json<Value>) -> Response {
    if body["stream"].as_bool().unwrap_or(false) {
        ([(CONTENT_TYPE, "text/event-stream")], UPSTREAM_SSE).into_response()
    } else {
        Json(json!({
            "model": "upstream-x",
            "choices": [{"message": {"content": "Hello from the gateway"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 4}
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
async fn openai_client_completes_chat_and_stream() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP openai_client_completes_chat_and_stream: WSF_OPENBAO_ADDR unset (G3 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // Seed an in-budget virtual key.
    let anchor = RustCryptoMlDsa87::generate("aog-g3-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_g3"),
            json!({ "token": in_budget_token(&anchor) }),
        )
        .await
        .expect("seed key");

    // Mock upstream + gateway state.
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

    // --- an off-the-shelf OpenAI client completes a chat (base-URL change only) ---
    let chat = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_g3")
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(chat.status(), 200, "chat status");
    let v: Value = chat.json().await.unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["model"], "gpt-4o-mini", "echoes the inbound model");
    assert_eq!(
        v["choices"][0]["message"]["content"],
        "Hello from the gateway"
    );
    assert_eq!(v["choices"][0]["message"]["role"], "assistant");
    assert_eq!(v["usage"]["total_tokens"], 9);

    // --- and a stream (SSE chat.completion.chunk frames ending with [DONE]) ---
    let stream_resp = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_g3")
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stream_resp.status(), 200, "stream status");
    let sse = stream_resp.text().await.unwrap();
    let mut text = String::new();
    let mut saw_done = false;
    let mut saw_finish = false;
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            saw_done = true;
            continue;
        }
        let frame: Value = serde_json::from_str(data).expect("chunk is JSON");
        assert_eq!(
            frame["object"], "chat.completion.chunk",
            "OpenAI stream object"
        );
        if let Some(d) = frame["choices"][0]["delta"]["content"].as_str() {
            text.push_str(d);
        }
        if frame["choices"][0]["finish_reason"] == "stop" {
            saw_finish = true;
        }
    }
    assert_eq!(text, "Hello from the gateway", "stream reassembles");
    assert!(saw_finish, "stream carried a finish_reason:stop frame");
    assert!(saw_done, "stream terminated with [DONE]");

    // --- /v1/models lists the routed model ---
    let models_resp = http.get(format!("{base}/v1/models")).send().await.unwrap();
    let mv: Value = models_resp.json().await.unwrap();
    assert_eq!(mv["object"], "list");
    assert_eq!(mv["data"][0]["id"], "gpt-4o-mini");

    // --- G5/G1: enforce (the production default) refuses PHI→cloud ---
    // The harness maps gpt-4o-mini to a (mock) cloud provider only. A PHI payload
    // classifies local-only; under the default enforce mode the gateway refuses the
    // cloud dispatch outright (403 + policy headers) instead of egressing it.
    // Shadow/report tag-don't-block semantics are the policy_modes gate's job.
    let phi = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_g3")
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "Patient John Doe, SSN 123-45-6789, diagnosis and plan"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        phi.status(),
        403,
        "PHI to a cloud-only target must be refused in enforce mode"
    );
    assert_eq!(
        phi.headers()
            .get("x-aog-policy")
            .and_then(|v| v.to_str().ok()),
        Some("deny"),
    );
    assert_eq!(
        phi.headers()
            .get("x-aog-policy-blocked")
            .and_then(|v| v.to_str().ok()),
        Some("true"),
    );
    let deny: Value = phi.json().await.unwrap();
    assert_eq!(deny["error"]["type"], "policy_denied");
    assert_eq!(deny["error"]["code"], "aog_enforce");

    // --- auth is enforced on the surface: no bearer → 401 ---
    let unauth = http
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({"model": "gpt-4o-mini", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401, "unauthenticated chat rejected");

    eprintln!(
        "G3 live gate PASSED against {addr} (OpenAI-wire chat + stream + models + auth + enforce PHI deny)"
    );
}
