//! `aogd` serves the authenticated `aog-apiserver` CRUD surface via
//! the `AppState::from_raft` seam, over the same node it drives consensus on:
//!
//! * `/healthz` stays open (liveness),
//! * a `/apis/**` request with no token is refused (fail-closed, doctrine I-4),
//! * a token minted under a ROGUE anchor is refused (the anchor binding holds),
//! * a token minted under the CONFIGURED anchor is admitted.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aogd::{Config, Daemon};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Duration as ChronoDuration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use tokio::net::TcpListener;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A `base64(json(TrustToken))` header value, signed under `signer`.
fn token_header(signer: &RustCryptoMlDsa87) -> String {
    let now = Utc::now();
    let token = TrustToken {
        token_id: "tok-vh5b".to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + ChronoDuration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: "tenant-loom".to_owned(),
        subject_id: None,
        subject_hash: "hmac:loom".to_owned(),
        service_identity: Some("aogctl".to_owned()),
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
    let minted = fabric_token::issue(token, signer).unwrap();
    BASE64.encode(serde_json::to_vec(&minted).unwrap())
}

async fn await_health(http: &reqwest::Client, base: &str) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(r) = http.get(format!("{base}/healthz")).send().await
            && r.status().is_success()
        {
            return true;
        }
        if start.elapsed() >= Duration::from_secs(5) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn aogd_authenticated_crud_via_from_raft() {
    let anchor = RustCryptoMlDsa87::generate("loom-vh5b-anchor").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        node_id: 1,
        data_dir: scratch("loom-vh5b-aogd"),
        listen: addr,
        advertise: format!("http://{addr}"),
        anchor_pubkey: Some(anchor.public_key().to_vec()),
        openbao: None,
        node_tls: None,
        allow_insecure_admin: false,
    };
    let daemon = Daemon::start(config).await.unwrap();
    let app = daemon.app();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let base = format!("http://{addr}");
    let http = reqwest::Client::new();
    assert!(
        await_health(&http, &base).await,
        "daemon health never came up"
    );

    let url = format!("{base}/apis/aog.islandmountain.io/v1/PolicyBundle");

    // No token -> 401: the CRUD surface is authenticated (fail-closed).
    let r = http.get(&url).send().await.unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "unauthenticated CRUD must be refused"
    );

    // A token minted under a ROGUE anchor -> 401: the anchor binding holds.
    let rogue = RustCryptoMlDsa87::generate("rogue-anchor").unwrap();
    let r = http
        .get(&url)
        .header("x-wsf-token", token_header(&rogue))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "a token under the wrong anchor must be refused"
    );

    // A token minted under the CONFIGURED anchor -> admitted (200 list).
    let r = http
        .get(&url)
        .header("x-wsf-token", token_header(&anchor))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        200,
        "a valid token must be admitted to the CRUD surface"
    );
}
