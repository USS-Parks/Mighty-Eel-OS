//! G1 gate — virtual key → trust-token resolution + **pre-flight budget** reject,
//! over HTTP, against live OpenBao KV.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (no `#[ignore]`). Self-provisions a `kv`
//! (v2) mount + an AppRole with read/write on `kv/data/aog/*`, seeds two virtual
//! keys (one in-budget, one exhausted) as `{ "token": <TrustToken> }` records,
//! spins the axum app on an ephemeral port, and drives it with a real HTTP
//! client. Without the env var it returns cleanly.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aog_gateway::{Gateway, GatewayConfig};
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "aog-gateway-test";
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
        "sys/policies/acl/aog-gateway-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,aog-gateway-test","token_ttl":"15m"})),
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

/// Build a signed token with a given id + optional budget.
fn token(signer: &RustCryptoMlDsa87, id: &str, budget: Option<Budget>) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: id.to_string(),
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
        budget,
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
    let hash = hex::encode(Sha256::digest(virtual_key.as_bytes()));
    format!("{KV_PREFIX}/{hash}")
}

#[tokio::test]
async fn virtual_key_resolves_and_budget_preflights() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP virtual_key_resolves_and_budget_preflights: WSF_OPENBAO_ADDR unset (G1 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // Trust anchor: signs the tokens; the gateway verifies with its public key.
    let anchor = RustCryptoMlDsa87::generate("aog-gateway-anchor").unwrap();
    let good = token(
        &anchor,
        "tok_g1_good",
        Some(Budget {
            token_cap: 100_000,
            tokens_spent: 10,
            ..Default::default()
        }),
    );
    let spent = token(
        &anchor,
        "tok_g1_spent",
        Some(Budget {
            token_cap: 1_000,
            tokens_spent: 1_000, // exhausted
            ..Default::default()
        }),
    );

    // Seed both virtual keys into live OpenBao KV.
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("approle login");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_good_g1"),
            json!({ "token": good }),
        )
        .await
        .expect("seed good key");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_spent_g1"),
            json!({ "token": spent }),
        )
        .await
        .expect("seed spent key");

    let gateway = Arc::new(Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: KV_PREFIX.to_string(),
        },
    ));

    // Direct resolution: the virtual key → the exact signed token + its tenant.
    let ctx = gateway
        .resolve_and_check("vk_good_g1", Utc::now())
        .await
        .expect("good key resolves");
    assert_eq!(ctx.token.token_id, "tok_g1_good");
    assert_eq!(ctx.tenant_id, "tenant-a");

    // Over-budget token → refused pre-flight (no model touched).
    let err = gateway
        .resolve_and_check("vk_spent_g1", Utc::now())
        .await
        .expect_err("spent key must be refused");
    assert!(
        matches!(err, aog_gateway::GatewayError::BudgetExhausted),
        "over-budget must be BudgetExhausted, got {err:?}"
    );

    // Over the HTTP surface.
    let app = aog_gateway::http::router(gateway.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()
        .unwrap();

    // healthz is open.
    let health = http.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);

    // Authorized preflight.
    let ok = http
        .post(format!("{base}/v1/preflight"))
        .bearer_auth("vk_good_g1")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "in-budget key authorized");
    let body: Value = ok.json().await.unwrap();
    assert_eq!(body["tenant_id"], "tenant-a");
    assert_eq!(body["authorized"], true);

    // Over-budget preflight → 402 Payment Required.
    let over = http
        .post(format!("{base}/v1/preflight"))
        .bearer_auth("vk_spent_g1")
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 402, "over-budget rejected pre-flight");

    // Unknown key → 401.
    let unknown = http
        .post(format!("{base}/v1/preflight"))
        .bearer_auth("vk_never_seeded")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 401, "unknown key unauthorized");

    server.abort();
    eprintln!(
        "G1 live gate PASSED against {addr} (virtual key → token resolution; over-budget → 402 pre-flight)"
    );
}
