//! W6 gate — the SDK round-trips **every** endpoint against a live stack.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` + `WSF_AWS_ENDPOINT` (OpenBao for tokens /
//! envelopes / receipts, Moto for the credential exchange). Provisions OpenBao
//! (tenant + transit key + broker root creds + one AppRole role with all
//! policies), constructs the four services behind `wsf_api::router`, spins it on
//! an ephemeral port, and drives every endpoint through `WsfClient`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};

use base64::Engine;
use fabric_contracts::Classification;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_api::client::WsfClient;
use wsf_api::{AppState, ExchangeReq, IssueReq, SealReq, UnsealReq};
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{LabelSpec, SealService, SealServiceConfig};

const ROLE: &str = "wsf-api-test";
const TENANT: &str = "wsf-api-tenant";
const TRANSIT_KEY: &str = "wsf-api-dek";
const CRED_PATH: &str = "kv/data/broker/aws-root";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}
fn aws_endpoint() -> Option<String> {
    std::env::var("WSF_AWS_ENDPOINT").ok()
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
    let _ = bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/mounts/transit",
        Some(json!({"type":"transit"})),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("transit/keys/{TRANSIT_KEY}"),
        Some(json!({"type":"aes256-gcm96"})),
    )
    .await;

    let policy = format!(
        "path \"kv/data/tenants/*\" {{ capabilities=[\"read\"] }}\npath \"kv/data/broker/*\" {{ capabilities=[\"read\"] }}\npath \"transit/encrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}\npath \"transit/decrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}"
    );
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-api-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-api-test","token_ttl":"15m"})),
    )
    .await;

    let attrs = json!({
        "tenant_id": TENANT, "display_name": "WSF API Tenant",
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
    bao(
        c,
        addr,
        tok,
        Method::POST,
        CRED_PATH,
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

#[tokio::test]
async fn sdk_round_trips_every_endpoint() {
    let (Some(addr), Some(aws)) = (openbao_addr(), aws_endpoint()) else {
        eprintln!(
            "SKIP sdk_round_trips_every_endpoint: set WSF_OPENBAO_ADDR + WSF_AWS_ENDPOINT (W6 live gate)"
        );
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

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-api-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            ob(),
            bridge_signer.clone(),
            BridgeConfig::new("2026.07.03.api", vec![5u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            ob(),
            Client::new(),
            BrokerConfig::new("us-east-1", &aws, CRED_PATH),
        )),
        seal: Arc::new(SealService::new(
            ob(),
            Arc::new(RustCryptoMlDsa87::generate("wsf-api-seal").unwrap()),
            SealServiceConfig {
                transit_key: TRANSIT_KEY.to_string(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-api-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(wsf_api::auth::LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(wsf_api::policy::StaticTenantPolicies::single_dev(
            TENANT,
            &["clinician"],
        )),
        grants: Arc::new(wsf_api::grants::StaticGrants::single_dev(
            TENANT,
            "aws-readonly",
            "arn:aws:iam::000000000000:role/wsf-api",
        )),
    };

    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let sdk = WsfClient::new(base);

    // OpenAPI published.
    assert!(sdk.openapi().await.unwrap().contains("WSF API"));

    // Token lifecycle.
    let token = sdk
        .issue(&IssueReq {
            requested_roles: vec!["clinician".to_string()],
            requested_models: vec![],
            budget: None,
        })
        .await
        .expect("issue");
    assert!(
        sdk.verify(&token).await.unwrap().valid,
        "issued token verifies"
    );

    // Attenuate with restriction-only intent; the child identity is derived
    // server-side from the authenticated parent (T2).
    let restrictions = fabric_token::TokenRestrictions {
        new_token_id: format!("{}-child", token.token_id),
        allowed_routes: Some(vec![]), // narrower
        ..fabric_token::TokenRestrictions::default()
    };
    let attenuated = sdk
        .attenuate(&token, &restrictions)
        .await
        .expect("attenuate");
    assert_eq!(
        attenuated.attenuation.parent_id,
        Some(token.token_id.clone())
    );
    assert_eq!(
        attenuated.tenant_id, token.tenant_id,
        "child inherits the parent's tenant (server-side, not caller-set)"
    );

    // Envelope lifecycle.
    let envelope = sdk
        .seal(&SealReq {
            token: token.clone(),
            plaintext_b64: base64::engine::general_purpose::STANDARD.encode(b"phi payload"),
            label: LabelSpec {
                classification: Classification::Restricted,
                compliance_scopes: vec![],
                origin: "api".to_string(),
                permitted_ops: vec!["unseal".to_string()],
                permitted_destinations: vec![],
                detected_entities: vec![],
            },
            envelope_id: "env-api".to_string(),
        })
        .await
        .expect("seal");
    let plaintext = sdk
        .unseal(&UnsealReq {
            token: token.clone(),
            envelope,
        })
        .await
        .expect("unseal");
    assert_eq!(plaintext, b"phi payload");

    // Credential exchange (Moto STS) — via a tenant-scoped grant id, not a raw ARN.
    let creds = sdk
        .exchange(&ExchangeReq {
            token: token.clone(),
            grant_id: "aws-readonly".to_string(),
        })
        .await
        .expect("exchange");
    assert!(!creds.access_key_id.is_empty());

    // Receipt query — issue + seal + unseal all carry the token id.
    let entries = sdk
        .receipts(Some("token_id"), Some(&token.token_id))
        .await
        .expect("receipts");
    assert!(
        entries.len() >= 3,
        "issue + seal + unseal receipts, got {}",
        entries.len()
    );

    server.abort();
    eprintln!(
        "W6 live gate PASSED against {addr} (+Moto {aws}): SDK round-tripped every endpoint; OpenAPI published"
    );
}
