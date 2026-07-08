//! K11 gate — `apply` then `get` round-trips a resource against a live apiserver;
//! an over-budget `apply` is rejected client-visibly (the 402 surfaces as a
//! `ClientError::Status`). The server runs in-process on an ephemeral port.

use std::net::SocketAddr;
use std::path::PathBuf;

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aogctl::{Client, ClientError};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use serde_json::{Value, json};

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn mint(signer: &RustCryptoMlDsa87, budget: Option<Budget>) -> TrustToken {
    let now = Utc::now();
    let token = TrustToken {
        token_id: "tok-cli".to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: "tenant-cli".to_owned(),
        subject_id: None,
        subject_hash: "hmac:cli".to_owned(),
        service_identity: Some("aogctl".to_owned()),
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Secret,
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
    fabric_token::issue(token, signer).unwrap()
}

/// Bind an apiserver on an ephemeral port and return its address + a token header.
async fn spawn(dir: &str, budget: Option<Budget>) -> (SocketAddr, String) {
    let signer = RustCryptoMlDsa87::generate("cli-anchor").unwrap();
    let auth = Authenticator::new(signer.public_key().to_vec());
    let state = AppState::bootstrap(1, fresh_dir(dir), auth, Sealer::generate().unwrap())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { aog_apiserver::serve(listener, state).await });
    let token = BASE64.encode(serde_json::to_vec(&mint(&signer, budget)).unwrap());
    (addr, token)
}

fn bundle(name: &str, version: u32) -> Value {
    json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": name },
        "spec": { "version": version },
    })
}

#[tokio::test]
async fn apply_then_get_roundtrips() {
    let (addr, token) = spawn("aogctl-roundtrip", None).await;
    let client = Client::new(format!("http://{addr}"), token);

    // apply (create), then get — round-trips.
    client
        .apply("PolicyBundle", &bundle("cfg", 1))
        .await
        .unwrap();
    let got = client.get("PolicyBundle", "cfg").await.unwrap();
    assert_eq!(got["spec"]["version"], 1);
    assert_eq!(got["metadata"]["name"], "cfg");

    // apply again — the create→409→replace path updates in place.
    client
        .apply("PolicyBundle", &bundle("cfg", 2))
        .await
        .unwrap();
    let got = client.get("PolicyBundle", "cfg").await.unwrap();
    assert_eq!(got["spec"]["version"], 2);

    // list + delete.
    let listed = client.list("PolicyBundle").await.unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    client.delete("PolicyBundle", "cfg").await.unwrap();
    let err = client.get("PolicyBundle", "cfg").await.unwrap_err();
    assert!(matches!(err, ClientError::Status { status: 404, .. }));
}

#[tokio::test]
async fn over_budget_apply_is_rejected_client_visibly() {
    let broke = Budget {
        token_cap: 100,
        tokens_spent: 100,
        ..Default::default()
    };
    let (addr, token) = spawn("aogctl-budget", Some(broke)).await;
    let client = Client::new(format!("http://{addr}"), token);

    let err = client
        .apply("PolicyBundle", &bundle("cfg", 1))
        .await
        .unwrap_err();
    match err {
        ClientError::Status { status, message } => {
            assert_eq!(status, 402, "an over-budget apply must surface a 402");
            assert!(!message.is_empty(), "the refusal is visible: {message}");
        }
        other => panic!("expected a client-visible status error, got {other:?}"),
    }
}
