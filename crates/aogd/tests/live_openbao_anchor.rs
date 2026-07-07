//! VH5b-c live gate — `aogd` sources its trust anchor **and** field-seal material
//! from a LIVE OpenBao, then serves the authenticated CRUD over it.
//!
//! Env-gated (no `#[ignore]`) on `WSF_OPENBAO_ADDR` — the `wsf-live` CI job's dev
//! OpenBao, or a local `docker run -e BAO_DEV_ROOT_TOKEN_ID=root -p 8200:8200
//! openbao/openbao`. It self-bootstraps an AppRole from the dev root token and
//! writes the trust record, so it needs only a bare dev OpenBao. Without the env
//! var it returns cleanly so the offline suite stays green. `WSF_OPENBAO_TOKEN`
//! overrides the root token (default `root`).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aogd::{Config, Daemon, OpenBaoTrust};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Duration as ChronoDuration, Utc};
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::json;
use tokio::net::TcpListener;

const ROLE: &str = "loom-aogd-test";
/// KV-v2 API path of the trust record the daemon reads.
const TRUST_PATH: &str = "kv/data/loom/trust";

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}

fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn bao(
    c: &Client,
    addr: &str,
    tok: &str,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let url = format!("{addr}/v1/{path}");
    let mut rb = c.request(method, &url).header("X-Vault-Token", tok);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    rb.send().await.expect("openbao request")
}

/// A `base64(json(TrustToken))` `x-wsf-token` header value, signed under `signer`.
fn token_header(signer: &RustCryptoMlDsa87) -> String {
    let now = Utc::now();
    let token = TrustToken {
        token_id: "tok-vh5bc-live".to_owned(),
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

async fn await_health(http: &Client, base: &str) -> bool {
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

/// Provision AppRole + KV from the dev root token and write the `blob` trust
/// record at `TRUST_PATH`. Returns `(role_id, secret_id)` for the node's login.
async fn provision(
    c: &Client,
    addr: &str,
    tok: &str,
    blob: &serde_json::Value,
) -> (String, String) {
    // Enable auth + secret engines (a 400 "already enabled" on reruns is fine).
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

    // Policy granting read on the Loom trust records.
    let policy = "path \"kv/data/loom/*\" { capabilities = [\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-aogd-test",
        Some(json!({ "policy": policy })),
    )
    .await;

    // Role bound to that policy.
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-aogd-test","token_ttl":"15m"})),
    )
    .await;

    let rid: serde_json::Value = bao(
        c,
        addr,
        tok,
        Method::GET,
        &format!("auth/approle/role/{ROLE}/role-id"),
        None,
    )
    .await
    .json()
    .await
    .expect("role-id json");
    let role_id = rid["data"]["role_id"]
        .as_str()
        .expect("role_id")
        .to_string();

    let sid: serde_json::Value = bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}/secret-id"),
        Some(json!({})),
    )
    .await
    .json()
    .await
    .expect("secret-id json");
    let secret_id = sid["data"]["secret_id"]
        .as_str()
        .expect("secret_id")
        .to_string();

    // Write the trust record (KV-v2 wraps the object under `data`).
    bao(
        c,
        addr,
        tok,
        Method::POST,
        TRUST_PATH,
        Some(json!({ "data": blob })),
    )
    .await;

    (role_id, secret_id)
}

#[tokio::test]
async fn aogd_trust_from_live_openbao() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP aogd_trust_from_live_openbao: WSF_OPENBAO_ADDR unset (VH5b-c live gate)");
        return;
    };

    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // The trust material the estate would custody: the WSF anchor (public only in
    // OpenBao; the test holds the secret to mint a client token) plus the
    // field-seal data key and child-mint signer.
    let anchor = RustCryptoMlDsa87::generate("loom-live-anchor").unwrap();
    let (seal_pk, seal_sk) = RustCryptoMlDsa87::keypair().unwrap();
    let blob = json!({
        "anchor_pubkey": BASE64.encode(anchor.public_key()),
        "seal_data_key": BASE64.encode([9u8; 32]),
        "seal_signer_pk": BASE64.encode(&seal_pk),
        "seal_signer_sk": BASE64.encode(&seal_sk),
    });
    let (role_id, secret_id) = provision(&c, &addr, &root_token(), &blob).await;

    // Start aogd sourcing its trust from OpenBao (no env anchor).
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sockaddr = listener.local_addr().unwrap();
    let config = Config {
        node_id: 1,
        data_dir: scratch("loom-vh5bc-aogd"),
        listen: sockaddr,
        advertise: format!("http://{sockaddr}"),
        anchor_pubkey: None,
        openbao: Some(OpenBaoTrust {
            address: addr.clone(),
            role_id,
            secret_id,
            trust_path: TRUST_PATH.to_owned(),
        }),
    };
    let daemon = Daemon::start(config)
        .await
        .expect("daemon starts with OpenBao-provisioned trust");
    let app = daemon.app();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let base = format!("http://{sockaddr}");
    let http = Client::new();
    assert!(
        await_health(&http, &base).await,
        "daemon health never came up"
    );

    let url = format!("{base}/apis/aog.islandmountain.io/v1/PolicyBundle");

    // No token -> 401: the CRUD surface is authenticated by the OpenBao anchor.
    let r = http.get(&url).send().await.unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "unauthenticated CRUD must be refused"
    );

    // A token minted under the OpenBao-custodied anchor -> admitted (200).
    let r = http
        .get(&url)
        .header("x-wsf-token", token_header(&anchor))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        200,
        "a token under the OpenBao-provisioned anchor must be admitted"
    );

    eprintln!("VH5b-c live gate PASSED against {addr}");
}
