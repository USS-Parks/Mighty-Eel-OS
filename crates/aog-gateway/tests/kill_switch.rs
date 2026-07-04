//! G9 live gate — budget kill-switch + revocation kill-switch against live OpenBao.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (the no-mock-only closure rule: budget/revocation
//! are trust-adjacent, so they ship a test against a real OpenBao). Drives the
//! `Gateway` resolve path directly:
//!
//! * **Budget** — a token with room at issue resolves; once accrued runtime spend
//!   (`record_spend`) crosses its cap, the next `resolve_and_check` is rejected
//!   `BudgetExhausted` — exhaustion blocks mid-session.
//! * **Revocation** — a valid token resolves; once a bridge-signed revocation
//!   snapshot naming it is written to the gateway's revocation KV path, the next
//!   `resolve_and_check` is rejected `Revoked` — the kill switch halts the next call.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use aog_gateway::{Gateway, GatewayConfig, GatewayError};
use chrono::{Duration, Utc};
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_revocation::RevocationSnapshot;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "aog-g9-test";
const KV_PREFIX: &str = "kv/data/aog/virtual-keys";
const REVOCATION_PATH: &str = "kv/data/aog/revocation";

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

/// Provision AppRole + KV v2 + a policy granting read/write on `kv/data/aog/*`
/// (covers both the virtual-keys and the revocation snapshot).
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
        &format!("sys/policies/acl/{ROLE}"),
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":format!("default,{ROLE}"),"token_ttl":"15m"})),
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

/// A signed token with a known id/subject and a chosen token-budget cap.
fn token(signer: &RustCryptoMlDsa87, token_id: &str, token_cap: u64) -> TrustToken {
    let now = Utc::now();
    let t = TrustToken {
        token_id: token_id.to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + Duration::minutes(15)).to_rfc3339(),
        issuer: "wsf-trust-bridge".to_string(),
        trust_bundle_version: "2026.07.g9".to_string(),
        tenant_id: "tenant-a".to_string(),
        subject_id: None,
        subject_hash: format!("hmac-sha256:{token_id}"),
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
        budget: Some(Budget {
            token_cap,
            tokens_spent: 0,
            ..Default::default()
        }),
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
    format!(
        "{KV_PREFIX}/{}",
        hex::encode(Sha256::digest(virtual_key.as_bytes()))
    )
}

#[tokio::test]
async fn budget_exhaustion_and_revocation_halt_the_next_call() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP kill_switch: WSF_OPENBAO_ADDR unset (G9 live gate)");
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;
    let anchor = RustCryptoMlDsa87::generate("aog-g9-anchor").unwrap();
    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    let vault_token = openbao.login().await.expect("login");

    // Seed a small-cap budget token and a separate revocation token.
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_bud"),
            json!({ "token": token(&anchor, "tok_bud", 100) }),
        )
        .await
        .expect("seed budget key");
    openbao
        .put_kv_data(
            &vault_token,
            &key_path("vk_rev"),
            json!({ "token": token(&anchor, "tok_rev", 1_000_000) }),
        )
        .await
        .expect("seed revocation key");

    let gateway = Gateway::new(
        openbao,
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: KV_PREFIX.to_string(),
        },
    )
    .with_revocation_path(REVOCATION_PATH.to_string());
    let now = Utc::now();

    // --- Budget kill-switch: exhaustion blocks mid-session -----------------
    gateway
        .resolve_and_check("vk_bud", now)
        .await
        .expect("in-budget resolves");
    gateway.record_spend("tok_bud", 80, 0, 0); // 80/100 — still room
    gateway
        .resolve_and_check("vk_bud", now)
        .await
        .expect("80/100 still resolves");
    gateway.record_spend("tok_bud", 40, 0, 0); // 120/100 — over cap
    match gateway.resolve_and_check("vk_bud", now).await {
        Err(GatewayError::BudgetExhausted) => {}
        other => panic!("expected BudgetExhausted after the cap is crossed, got {other:?}"),
    }

    // --- Revocation kill-switch: revoke → the next call is refused ----------
    // No snapshot yet → nothing revoked → resolves.
    gateway
        .resolve_and_check("vk_rev", now)
        .await
        .expect("valid token resolves before revocation");

    // Write a bridge-signed snapshot naming the token, at the gateway's path.
    let mut snap = RevocationSnapshot::new(
        "snap-g9",
        now.to_rfc3339(),
        (now + Duration::hours(1)).to_rfc3339(),
    );
    snap.revoked_tokens.push("tok_rev".to_string());
    let signed = fabric_revocation::sign(snap, &anchor).expect("sign snapshot");
    put_snapshot(&addr, &c, &root_token(), &signed).await;

    match gateway.resolve_and_check("vk_rev", now).await {
        Err(GatewayError::Revoked) => {}
        other => panic!("expected Revoked after the snapshot is written, got {other:?}"),
    }
}

/// Write the signed snapshot to the gateway's revocation KV path (KV v2 shape).
async fn put_snapshot(addr: &str, c: &Client, tok: &str, snapshot: &RevocationSnapshot) {
    bao(
        c,
        addr,
        tok,
        Method::POST,
        REVOCATION_PATH,
        Some(json!({ "data": { "snapshot": snapshot } })),
    )
    .await;
}
