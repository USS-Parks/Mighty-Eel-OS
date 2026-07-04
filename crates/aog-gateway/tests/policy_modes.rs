//! G6 gate — the **same** request differs across modes; **shadow never blocks**;
//! **enforce** blocks a PHI→cloud egress.
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. Seeds an in-budget virtual key, stands a mock
//! (cloud) OpenAI upstream behind the gateway, and drives the same PHI chat under
//! two `AppState`s that differ only in `PolicyMode`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aog_gateway::app::{AppState, ModelMap, Target};
use aog_gateway::policy::PolicyMode;
use aog_gateway::provider::Registry;
use aog_gateway::provider::openai::OpenAiProvider;
use aog_gateway::{Gateway, GatewayConfig};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "aog-g6-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const PHI: &str = "Patient John Doe, SSN 123-45-6789, diagnosis and treatment plan";

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
        "sys/policies/acl/aog-g6-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-g6-test","token_ttl":"15m"})),
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
        token_id: "tok_g6".to_string(),
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
        allowed_routes: vec![],
        allowed_models: vec![],
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

async fn upstream(Json(_body): Json<Value>) -> Response {
    Json(json!({
        "model": "upstream-x",
        "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}
    }))
    .into_response()
}

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn phi_chat(http: &Client, base: &str) -> reqwest::Response {
    http.post(format!("{base}/v1/chat/completions"))
        .bearer_auth("vk_g6")
        .json(&json!({ "model": "gpt-4o-mini", "messages": [{"role": "user", "content": PHI}] }))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn modes_change_the_same_phi_request() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP modes_change_the_same_phi_request: WSF_OPENBAO_ADDR unset (G6 live gate)");
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("aog-g6-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_g6"),
            json!({ "token": in_budget_token(&anchor) }),
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
    let registry = Arc::new(registry);
    // "gpt-4o-mini" maps to the cloud "openai" provider — a PHI request here is a
    // classified-data cloud egress.
    let models =
        Arc::new(ModelMap::new().route("gpt-4o-mini", Target::new("openai", "upstream-x")));

    let shadow = AppState::new(gateway.clone(), registry.clone(), models.clone());
    let enforce = AppState::new(gateway, registry, models).with_mode(PolicyMode::Enforce);

    let shadow_base = spawn(aog_gateway::surface_openai::router(shadow)).await;
    let enforce_base = spawn(aog_gateway::surface_openai::router(enforce)).await;
    let http = Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()
        .unwrap();

    // Shadow: the PHI request is decided (deny) + logged, but NEVER blocked.
    let s = phi_chat(&http, &shadow_base).await;
    assert_eq!(s.status(), 200, "shadow never blocks");
    assert_eq!(
        s.headers()
            .get("x-aog-policy-mode")
            .and_then(|v| v.to_str().ok()),
        Some("shadow")
    );
    assert_eq!(
        s.headers()
            .get("x-aog-policy")
            .and_then(|v| v.to_str().ok()),
        Some("deny"),
        "PHI→cloud is a deny (deny-wins)"
    );
    assert_eq!(
        s.headers()
            .get("x-aog-policy-blocked")
            .and_then(|v| v.to_str().ok()),
        Some("false")
    );

    // Enforce: the identical request is blocked before dispatch (403).
    let e = phi_chat(&http, &enforce_base).await;
    assert_eq!(e.status(), 403, "enforce blocks the PHI→cloud egress");
    assert_eq!(
        e.headers()
            .get("x-aog-policy-mode")
            .and_then(|v| v.to_str().ok()),
        Some("enforce")
    );
    assert_eq!(
        e.headers()
            .get("x-aog-policy-blocked")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let eb: Value = e.json().await.unwrap();
    assert_eq!(eb["error"]["type"], "policy_denied");

    // A benign request under enforce is NOT blocked.
    let benign = http
        .post(format!("{enforce_base}/v1/chat/completions"))
        .bearer_auth("vk_g6")
        .json(&json!({ "model": "gpt-4o-mini", "messages": [{"role": "user", "content": "What is the capital of France?"}] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        benign.status(),
        200,
        "enforce does not block a benign request"
    );
    assert_eq!(
        benign
            .headers()
            .get("x-aog-policy")
            .and_then(|v| v.to_str().ok()),
        Some("allow")
    );

    eprintln!(
        "G6 live gate PASSED against {addr} (shadow never blocks; enforce blocks PHI→cloud; deny-wins)"
    );
}
