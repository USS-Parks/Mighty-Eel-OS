//! W8 gate — trust token → scoped Azure AD token, with the broker's app creds
//! custodied in **live OpenBao** and the OAuth2 token endpoint served by a
//! **local mock** (no free Azure AD emulator; a real-Azure run is owner-gated).
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. The mock requires a non-empty `scope` and
//! returns a 3600s Azure token; the trust token's TTL is 15 min, so the test
//! proves the brokered credential's effective expiry is **capped to the token**
//! (≈900s, not Azure's 3600) — TTL enforced — and scope flows end-to-end.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use axum::{Json, Router, extract::Path, http::StatusCode, routing::post};
use chrono::{Duration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_broker::{AzureBroker, AzureBrokerConfig, AzureGrantScope};

const ROLE: &str = "wsf-azure-test";
const CREDS_PATH: &str = "kv/data/broker/azure";
const SCOPE: &str = "https://storage.azure.com/.default";

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
    let policy = "path \"kv/data/broker/*\" { capabilities=[\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-azure-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-azure-test","token_ttl":"15m"})),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        CREDS_PATH,
        Some(json!({ "data": { "client_id": "app-guid", "client_secret": "app-secret" } })),
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

/// Mock Azure AD token endpoint: require a non-empty scope; return a 3600s token.
async fn mock_token(
    Path(_tenant): Path<String>,
    body: String,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = body
        .split('&')
        .find_map(|kv| kv.strip_prefix("scope="))
        .unwrap_or("");
    if scope.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "scope required".to_string()));
    }
    Ok(Json(json!({
        "token_type": "Bearer",
        "expires_in": 3600,
        "access_token": "eyJ0.mock.azure",
    })))
}

fn signed_token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: "tok_azure-e2e".to_string(),
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
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(t, signer).unwrap()
}

#[tokio::test]
async fn azure_broker_caps_ttl_to_token() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP azure_broker_caps_ttl_to_token: WSF_OPENBAO_ADDR unset (W8 live gate)");
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let app = Router::new().route("/{tenant}/oauth2/v2.0/token", post(mock_token));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let broker = AzureBroker::new(
        openbao,
        Client::new(),
        AzureBrokerConfig::new(&mock_base, "test-tenant-id", CREDS_PATH),
    );

    let signer = RustCryptoMlDsa87::generate("wsf-azure-test-key").unwrap();
    let verifier = fabric_crypto::providers::MlDsa87Verifier;
    let token = signed_token(&signer);
    let now = Utc::now();

    let creds = broker
        .acquire_token(
            &token,
            &verifier,
            signer.public_key(),
            &AzureGrantScope::new(SCOPE),
            now,
        )
        .await
        .expect("acquire azure token");
    assert!(!creds.access_token.is_empty());
    // Azure returned 3600s but the trust token has ~900s left → capped to token.
    let secs = (creds.expires_at - now).num_seconds();
    assert!(
        secs <= 1000,
        "effective expiry capped to the token TTL, got {secs}s"
    );
    assert!(
        secs > 800,
        "effective expiry near the token TTL, got {secs}s"
    );

    // Wrong key → refused before any Azure/OpenBao call.
    let wrong = RustCryptoMlDsa87::generate("wrong").unwrap();
    let denied = broker
        .acquire_token(
            &token,
            &verifier,
            wrong.public_key(),
            &AzureGrantScope::new(SCOPE),
            now,
        )
        .await
        .unwrap_err();
    assert!(matches!(denied, wsf_broker::BrokerError::TokenRejected(_)));

    server.abort();
    eprintln!(
        "W8 live gate PASSED against {addr}: scoped Azure token minted, effective TTL capped to the trust token"
    );
}
