//! W7 gate — trust token → scoped GCP access token, with the broker's Google
//! bearer custodied in **live OpenBao** and the IAM-Credentials
//! `generateAccessToken` contract served by a **local mock** (no free GCP
//! emulator exists; a real-GCP run is owner-gated).
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. The mock enforces that a non-empty `scope`
//! is present and echoes the requested `lifetime` into `expireTime`, so the test
//! proves scope + TTL flow end-to-end over HTTP.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use axum::{Json, Router, extract::Path, http::StatusCode, routing::post};
use chrono::{Duration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_broker::{GcpBroker, GcpBrokerConfig};

const ROLE: &str = "wsf-gcp-test";
const BEARER_PATH: &str = "kv/data/broker/gcp-bearer";

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
        "sys/policies/acl/wsf-gcp-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-gcp-test","token_ttl":"15m"})),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        BEARER_PATH,
        Some(json!({ "data": { "bearer": "ya29.broker-mock-bearer" } })),
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

/// Mock `generateAccessToken`: require a non-empty scope; echo lifetime → expiry.
async fn mock_generate(
    Path(target): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !target.ends_with(":generateAccessToken") {
        return Err((StatusCode::NOT_FOUND, "unknown method".to_string()));
    }
    let scopes = body
        .get("scope")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if scopes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "scope required".to_string()));
    }
    let lifetime = body
        .get("lifetime")
        .and_then(Value::as_str)
        .unwrap_or("0s")
        .trim_end_matches('s')
        .parse::<i64>()
        .unwrap_or(0);
    let expire = Utc::now() + Duration::seconds(lifetime);
    Ok(Json(json!({
        "accessToken": format!("ya29.mock-{lifetime}"),
        "expireTime": expire.to_rfc3339(),
    })))
}

fn signed_token(signer: &RustCryptoMlDsa87) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: "tok_gcp-e2e".to_string(),
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
async fn gcp_broker_mints_scoped_token() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP gcp_broker_mints_scoped_token: WSF_OPENBAO_ADDR unset (W7 live gate)");
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // Local mock of the IAM Credentials endpoint.
    let app = Router::new().route(
        "/v1/projects/-/serviceAccounts/{target}",
        post(mock_generate),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let broker = GcpBroker::new(
        openbao,
        Client::new(),
        GcpBrokerConfig::new(&mock_base, BEARER_PATH),
    );

    let signer = RustCryptoMlDsa87::generate("wsf-gcp-test-key").unwrap();
    let verifier = fabric_crypto::providers::MlDsa87Verifier;
    let token = signed_token(&signer);
    let now = Utc::now();
    let scopes = vec!["https://www.googleapis.com/auth/cloud-platform".to_string()];

    let creds = broker
        .generate_access_token(
            &token,
            &verifier,
            signer.public_key(),
            "sa@proj.iam.gserviceaccount.com",
            &scopes,
            now,
        )
        .await
        .expect("mint gcp token");
    assert!(creds.access_token.starts_with("ya29.mock-"));
    assert!(creds.expire_time > now, "token expires in the future");
    assert!(
        (creds.expire_time - now).num_seconds() <= 3600,
        "lifetime bounded"
    );

    // Empty scope → the mock rejects → broker surfaces an STS error.
    let empty: Vec<String> = vec![];
    let err = broker
        .generate_access_token(
            &token,
            &verifier,
            signer.public_key(),
            "sa@proj.iam.gserviceaccount.com",
            &empty,
            now,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, wsf_broker::BrokerError::Sts(_)),
        "empty scope rejected"
    );

    // Wrong key → refused before any GCP/OpenBao call.
    let wrong = RustCryptoMlDsa87::generate("wrong").unwrap();
    let denied = broker
        .generate_access_token(
            &token,
            &verifier,
            wrong.public_key(),
            "sa@proj.iam.gserviceaccount.com",
            &scopes,
            now,
        )
        .await
        .unwrap_err();
    assert!(matches!(denied, wsf_broker::BrokerError::TokenRejected(_)));

    server.abort();
    eprintln!(
        "W7 live gate PASSED against {addr}: scoped GCP token minted (scope+TTL enforced), fail-closed on bad scope/token"
    );
}
