//! W9 gate — provision → issue → deprovision → **revoked everywhere**.
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. Provisions a tenant via the admin (minting a
//! per-tenant subject-HMAC key), issues a real token through the bridge (which
//! uses that per-tenant key), deprovisions the tenant (deletes the record + signs
//! a revocation snapshot), and proves the token is now refused **offline** by a
//! Ring-3 cache that applies the snapshot.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use chrono::Utc;
use fabric_cache::TtlPolicy;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_cache::Ring3Cache;
use wsf_tenants::{TenantAdmin, TenantAdminConfig, TenantSpec};

const ROLE: &str = "wsf-tenants-test";
const TENANT: &str = "wsf-tenants-demo";
const TTL: TtlPolicy = TtlPolicy {
    soft_ttl_secs: 3600,
    hard_ttl_secs: 86_400,
};

fn openbao_addr() -> Option<String> {
    std::env::var("WSF_OPENBAO_ADDR").ok()
}
fn root_token() -> String {
    std::env::var("WSF_OPENBAO_TOKEN").unwrap_or_else(|_| "root".to_string())
}
fn now_secs() -> u64 {
    u64::try_from(Utc::now().timestamp()).unwrap()
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

/// Provision an AppRole role with admin capabilities on tenants + revocations.
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
    let policy = "path \"kv/data/tenants/*\" { capabilities=[\"create\",\"update\",\"read\"] }\npath \"kv/metadata/tenants/*\" { capabilities=[\"delete\",\"read\"] }\npath \"kv/data/revocations/*\" { capabilities=[\"create\",\"update\",\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-tenants-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-tenants-test","token_ttl":"15m"})),
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
async fn provision_issue_deprovision_revokes_everywhere() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP provision_issue_deprovision_revokes_everywhere: WSF_OPENBAO_ADDR unset (W9 live gate)"
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

    let anchor_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-tenants-anchor").unwrap());
    let anchor = anchor_signer.public_key().to_vec();

    // Provision (mints a per-tenant subject-HMAC key).
    let admin = TenantAdmin::new(ob(), anchor_signer.clone(), TenantAdminConfig::new());
    let now = Utc::now();
    let record = admin
        .provision(
            &TenantSpec {
                tenant_id: TENANT.to_string(),
                display_name: "WSF Tenants Demo".to_string(),
                compliance_scopes: vec!["hipaa".to_string()],
                default_allowed_routes: vec!["local_only".to_string()],
                max_data_classification: "restricted".to_string(),
            },
            now,
        )
        .await
        .expect("provision");
    assert_eq!(
        record.subject_hmac_key.len(),
        64,
        "per-tenant HMAC key minted"
    );

    // Issue a real token — the bridge uses the tenant's per-tenant HMAC key.
    let bridge = TrustBridge::new(
        ob(),
        anchor_signer.clone(),
        BridgeConfig::new("2026.07.03.ten", vec![6u8; 32]),
    );
    let token = bridge
        .issue_token(&IssueTokenRequest::new(
            TENANT,
            "clinician-9",
            vec!["clinician".to_string()],
        ))
        .await
        .expect("issue token");
    // The subject_hash must be derived from the TENANT's key, not the bridge config key.
    let key = hex::decode(&record.subject_hmac_key).unwrap();
    assert_eq!(
        token.subject_hash,
        fabric_proof::hmac_subject(&key, "clinician-9").unwrap(),
        "bridge used the per-tenant HMAC key"
    );

    // Deprovision — deletes the record + signs a revocation snapshot.
    let snapshot = admin
        .deprovision(
            TENANT,
            vec![token.token_id.clone()],
            vec![token.subject_hash.clone()],
            Utc::now(),
        )
        .await
        .expect("deprovision");
    assert!(snapshot.revoked_tokens.contains(&token.token_id));
    assert!(admin.get(TENANT).await.is_err(), "record deleted");

    // Revoked everywhere: a Ring-3 cache that applies the snapshot refuses the token offline.
    let mut cache = Ring3Cache::new(anchor.clone(), TTL);
    cache
        .refresh(snapshot, now_secs())
        .expect("apply revocation");
    let decision = cache.decide(&token, Utc::now());
    assert!(
        !decision.token_valid,
        "deprovisioned tenant's token is revoked"
    );
    assert!(decision.reason.contains("revoked"));

    eprintln!(
        "W9 live gate PASSED against {addr}: provision→issue(per-tenant HMAC)→deprovision→revoked offline"
    );
}
