//! B6 live gate — credential exchange is grant-scoped, never raw-ARN
//! (AF-004). Black-box against the real WSF API + live OpenBao + Moto STS.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (+ Moto at 127.0.0.1:5566). Proves: an
//! approved grant brokers scoped creds; a smuggled `role_arn` is rejected
//! (422, deny_unknown_fields); an unknown/cross-tenant grant is denied (403).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};

use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use wsf_api::AppState;
use wsf_api::auth::LocalDevAuthenticator;
use wsf_api::grants::StaticGrants;
use wsf_api::policy::StaticTenantPolicies;
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{SealService, SealServiceConfig};

const ROLE: &str = "wsf-grant-test";
const TENANT: &str = "wsf-grant-tenant";
const APPROVED_ARN: &str = "arn:aws:iam::000000000000:role/approved";

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
    bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type":"approle"})),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/mounts/kv",
        Some(json!({"type":"kv","options":{"version":"2"}})),
    )
    .await;
    let policy = "path \"kv/data/tenants/*\" { capabilities=[\"read\"] }\npath \"kv/data/broker/*\" { capabilities=[\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-grant-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-grant-test","token_ttl":"15m"})),
    )
    .await;
    let attrs = json!({
        "tenant_id": TENANT, "display_name": TENANT,
        "compliance_scopes": ["hipaa"], "default_allowed_routes": ["local_only"],
        "max_data_classification": "restricted"
    });
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("kv/data/tenants/{TENANT}"),
        Some(json!({ "data": { "attributes": attrs.to_string() } })),
    )
    .await;
    // Broker root creds for Moto STS.
    bao(
        c,
        addr,
        tok,
        Method::POST,
        "kv/data/broker/aws-root",
        Some(json!({ "data": { "access_key_id": "test", "secret_access_key": "test" } })),
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
    .expect("role-id");
    let role_id = rid["data"]["role_id"].as_str().unwrap().to_string();
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
    .expect("secret-id");
    let secret_id = sid["data"]["secret_id"].as_str().unwrap().to_string();
    (role_id, secret_id)
}

#[tokio::test]
async fn exchange_is_grant_scoped_not_raw_arn() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP broker_grant: set WSF_OPENBAO_ADDR (B6 live gate)");
        return;
    };
    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;
    let ob = || {
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .unwrap()
    };

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-grant-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            ob(),
            bridge_signer,
            BridgeConfig::new("2026.07.grant", vec![3u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            ob(),
            Client::new(),
            BrokerConfig::new(
                "us-east-1",
                "http://127.0.0.1:5566",
                "kv/data/broker/aws-root",
            ),
        )),
        seal: Arc::new(SealService::new(
            ob(),
            Arc::new(RustCryptoMlDsa87::generate("wsf-grant-seal").unwrap()),
            SealServiceConfig {
                transit_key: "wsf-grant-dek".into(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-grant-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(StaticTenantPolicies::single_dev(TENANT, &["clinician"])),
        // One approved grant for this tenant.
        grants: Arc::new(StaticGrants::single_dev(
            TENANT,
            "aws-approved",
            APPROVED_ARN,
        )),
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
    };
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = Client::new();
    // Issue a token with a resource caveat so the session policy is non-empty.
    let issued: Value = http
        .post(format!("{base}/v1/tokens/issue"))
        .json(&json!({ "requested_roles": ["clinician"] }))
        .send()
        .await
        .expect("issue")
        .json()
        .await
        .expect("issue json");
    let token = issued["token"].clone();

    let exchange = |body: Value| {
        let http = http.clone();
        let url = format!("{base}/v1/credentials/exchange");
        async move {
            http.post(url)
                .json(&body)
                .send()
                .await
                .expect("exchange req")
        }
    };

    // 1. Approved grant → 200 with scoped creds.
    let ok = exchange(json!({ "token": token, "grant_id": "aws-approved" })).await;
    assert_eq!(ok.status(), StatusCode::OK, "approved grant brokers creds");
    let creds: Value = ok.json().await.unwrap();
    assert!(!creds["access_key_id"].as_str().unwrap_or("").is_empty());

    // 2. A smuggled raw role_arn is rejected outright (422) — the caller cannot
    //    name a cloud identity anymore (the AF-004 input is gone).
    let raw = exchange(json!({
        "token": token,
        "role_arn": "arn:aws:iam::999999999999:role/attacker"
    }))
    .await;
    assert_eq!(
        raw.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "raw role_arn is rejected"
    );

    // 3. An unknown grant id is denied (403) — no adjacent/guessed grant works.
    let unknown = exchange(json!({ "token": token, "grant_id": "aws-admin" })).await;
    assert_eq!(
        unknown.status(),
        StatusCode::FORBIDDEN,
        "unknown grant is denied"
    );

    println!(
        "B6 live gate PASSED against {addr}: grant-scoped exchange; raw ARN 422; unknown grant 403"
    );
}
