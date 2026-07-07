//! W3 gate — seal / unseal **over HTTP** against live OpenBao **Transit**.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (no `#[ignore]`). Self-provisions the transit
//! engine + an aes256-gcm96 key + an AppRole role (transit encrypt/decrypt) from
//! the dev root token, spins the axum app on an ephemeral port, and drives it
//! with a real HTTP client. Without the env var it returns cleanly.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use base64::Engine;
use chrono::{Duration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_seal::{SealService, SealServiceConfig};

const ROLE: &str = "wsf-seal-test";
const TRANSIT_KEY: &str = "wsf-seal-dek";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}
fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
fn unb64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
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
        "path \"transit/encrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}\npath \"transit/decrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}"
    );
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-seal-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-seal-test","token_ttl":"15m"})),
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

fn token(signer: &RustCryptoMlDsa87, clearance: Classification) -> TrustToken {
    token_for(signer, clearance, "tenant-a")
}

fn token_for(signer: &RustCryptoMlDsa87, clearance: Classification, tenant: &str) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: format!("tok_seal-{tenant}-{clearance:?}"),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::minutes(15)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.03".to_string(),
        tenant_id: tenant.to_string(),
        subject_id: None,
        subject_hash: "hmac-sha256:demo".to_string(),
        service_identity: None,
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: clearance,
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
async fn seal_unseal_over_http_against_live_openbao() {
    let Some(bao_addr) = openbao_addr() else {
        eprintln!(
            "SKIP seal_unseal_over_http_against_live_openbao: WSF_OPENBAO_ADDR unset (W3 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &bao_addr, &root_token()).await;

    // Trust anchor: the token-issuing signer; the service verifies with its pubkey.
    let token_signer = RustCryptoMlDsa87::generate("wsf-seal-token-anchor").unwrap();
    let service_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-seal-service").unwrap());

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&bao_addr, role_id, secret_id)).unwrap();
    let service = Arc::new(SealService::new(
        openbao,
        service_signer,
        SealServiceConfig {
            transit_key: TRANSIT_KEY.to_string(),
            token_public_key: token_signer.public_key().to_vec(),
        },
    ));

    // Spin the axum app on an ephemeral port.
    let app = wsf_seal::http::router(service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()
        .unwrap();

    // 1. Seal — token cleared to Secret; label Restricted, permits unseal.
    let hi = token(&token_signer, Classification::Secret);
    let seal_body = json!({
        "token": hi.clone(),
        "plaintext_b64": b64(b"regulated payload"),
        "label": {"classification":"restricted","origin":"ingest","permitted_ops":["unseal"]},
        "envelope_id": "env-1"
    });
    let seal_resp = http
        .post(format!("{base}/seal"))
        .json(&seal_body)
        .send()
        .await
        .unwrap();
    assert_eq!(seal_resp.status(), 200, "seal status");
    let seal_json: Value = seal_resp.json().await.unwrap();
    let envelope = seal_json["envelope"].clone();
    // The data key is a real OpenBao-Transit wrap.
    assert!(
        envelope["seal"]["data_key_wrapped"]
            .as_str()
            .unwrap()
            .starts_with("vault:v1:"),
        "data key must be transit-wrapped"
    );

    // 2. Unseal with the cleared token -> plaintext round-trips.
    let unseal_body = json!({ "token": hi.clone(), "envelope": envelope.clone() });
    let unseal_resp = http
        .post(format!("{base}/unseal"))
        .json(&unseal_body)
        .send()
        .await
        .unwrap();
    assert_eq!(unseal_resp.status(), 200, "unseal status");
    let unseal_json: Value = unseal_resp.json().await.unwrap();
    assert_eq!(
        unb64(unseal_json["plaintext_b64"].as_str().unwrap()),
        b"regulated payload"
    );

    // 3. Unauthorized unseal — valid but under-cleared (Public) token -> 403 + deny receipt.
    let lo = token(&token_signer, Classification::Public);
    let deny_body = json!({ "token": lo, "envelope": envelope.clone() });
    let deny_resp = http
        .post(format!("{base}/unseal"))
        .json(&deny_body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        deny_resp.status(),
        403,
        "under-cleared unseal must be denied"
    );

    // 4. E7 cross-tenant — a fully-cleared token from ANOTHER tenant is denied
    //    before any Transit decrypt (AF-003). Same clearance as the sealer, so
    //    only the tenant binding stops it.
    let other = token_for(&token_signer, Classification::Secret, "tenant-b");
    let cross_body = json!({ "token": other, "envelope": envelope.clone() });
    let cross_resp = http
        .post(format!("{base}/unseal"))
        .json(&cross_body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_resp.status(),
        403,
        "cross-tenant unseal must be denied (E7/AF-003)"
    );

    // Receipts: the chain verifies and records seal-allow, unseal-allow, unseal-deny.
    let links = service.receipt_links();
    fabric_proof::verify_chain(&links).expect("receipt chain verifies");
    let receipts = service.receipts_snapshot();
    assert!(
        receipts
            .iter()
            .any(|r| r.op == "seal" && r.decision == "allow")
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.op == "unseal" && r.decision == "allow")
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.op == "unseal" && r.decision == "deny"),
        "deny receipt present"
    );

    // Two distinct deny receipts now: under-cleared + cross-tenant.
    assert!(
        service
            .receipts_snapshot()
            .iter()
            .filter(|r| r.op == "unseal" && r.decision == "deny")
            .count()
            >= 2,
        "under-cleared and cross-tenant denials both receipted"
    );

    server.abort();
    eprintln!(
        "W3/E7 live gate PASSED against {bao_addr} (transit-wrapped seal + HTTP unseal + under-cleared + cross-tenant denials)"
    );
}
