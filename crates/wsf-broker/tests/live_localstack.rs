//! W2 gate — trust token → scoped AWS credentials against **LocalStack** STS,
//! with the broker's root creds custodied in **live OpenBao**.
//!
//! Env-gated (no `#[ignore]`): runs only when BOTH `WSF_OPENBAO_ADDR` and
//! `WSF_AWS_ENDPOINT` are set (the `wsf-live` CI job provides both). It
//! self-bootstraps OpenBao (an AppRole role with kv read + a root-cred record)
//! from the dev root token, and points STS at LocalStack. Without the env vars
//! it returns cleanly so the offline suite stays green.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use chrono::{Duration as ChronoDuration, Utc};
use fabric_contracts::{
    Attenuation, Caveat, CaveatType, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_broker::{AwsStsBroker, BrokerConfig};

const ROLE: &str = "wsf-broker-test";
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
    body: Option<serde_json::Value>,
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
        .expect("openbao request")
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
    let policy = "path \"kv/data/broker/*\" { capabilities = [\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-broker-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-broker-test","token_ttl":"15m"})),
    )
    .await;
    let rid: serde_json::Value = serde_json::from_str(
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
    let sid: serde_json::Value = serde_json::from_str(
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

    // Root creds — LocalStack accepts any creds (it does not verify SigV4).
    bao(
        c,
        addr,
        tok,
        Method::POST,
        CRED_PATH,
        Some(json!({ "data": { "access_key_id": "test", "secret_access_key": "test" } })),
    )
    .await;

    (role_id, secret_id)
}

fn signed_token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let now = Utc::now();
    let exp = now + ChronoDuration::minutes(15);
    let tok = TrustToken {
        token_id: "tok_broker-e2e".to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: exp.to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.03.test".to_string(),
        tenant_id: "tenant-a".to_string(),
        subject_id: None,
        subject_hash: "hmac-sha256:demo".to_string(),
        service_identity: None,
        identity_id: None,
        roles: vec!["clinician".to_string()],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation {
            parent_id: None,
            caveats: vec![Caveat {
                caveat_type: CaveatType::ResourcePrefix,
                value: "arn:aws:s3:::wsf-demo/*".to_string(),
            }],
            depth: 0,
        },
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(tok, signer).unwrap()
}

#[tokio::test]
async fn broker_scopes_credentials_against_localstack() {
    let (Some(bao_addr), Some(aws)) = (openbao_addr(), aws_endpoint()) else {
        eprintln!(
            "SKIP broker_scopes_credentials_against_localstack: set WSF_OPENBAO_ADDR + WSF_AWS_ENDPOINT (W2 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &bao_addr, &root_token()).await;

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&bao_addr, role_id, secret_id)).unwrap();
    let broker = AwsStsBroker::new(
        openbao,
        Client::new(),
        BrokerConfig::new("us-east-1", &aws, CRED_PATH),
    );

    let signer = RustCryptoMlDsa87::generate("wsf-broker-test-key").unwrap();
    let token = signed_token(&signer);
    let now = Utc::now();

    let creds = broker
        .broker_credentials(
            &token,
            &MlDsa87Verifier,
            signer.public_key(),
            "arn:aws:iam::000000000000:role/wsf-demo",
            now,
        )
        .await
        .expect("assume role via localstack");

    assert!(!creds.access_key_id.is_empty(), "temp access key returned");
    assert!(!creds.session_token.is_empty(), "session token returned");
    // Creds expire in the future, bounded by the token TTL (clamped to the STS
    // window). The token had 15 min TTL -> ~900s duration.
    assert!(creds.expiration > now, "creds must expire in the future");
    assert!(
        (creds.expiration - now).num_seconds() <= 3600,
        "duration bounded by the STS ceiling"
    );

    eprintln!(
        "W2 live gate PASSED: brokered {} (exp {}) via {}",
        creds.access_key_id, creds.expiration, aws
    );
}
