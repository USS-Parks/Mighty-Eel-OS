//! W5 gate (live-material integration) — sync the Ring-3 cache from **real**
//! bridge material and confirm the offline decisions.
//!
//! W5's decision path is offline **by design** (the appliance runs with no
//! network), so the crate's unit tests — real ML-DSA crypto, nothing mocked —
//! are the primary gate proof. This test additionally issues a **real** token
//! and a **real** bridge-signed revocation snapshot against live OpenBao, syncs
//! them into the cache (Ring 2 → Ring 3), and checks: fresh → cloud allowed;
//! air-gap → narrowed to local-only; a revoked token → denied offline.
//! Env-gated on `WSF_OPENBAO_ADDR`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use chrono::Utc;
use fabric_contracts::Route;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_revocation::RevocationSnapshot;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_cache::Ring3Cache;

use fabric_cache::TtlPolicy;

const ROLE: &str = "wsf-cache-test";
const TENANT: &str = "wsf-cache-tenant";
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

/// Provision a tenant whose default route is **cloud_allowed** (so the issued
/// token can be narrowed), plus an AppRole role with kv read.
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
    let policy = "path \"kv/data/tenants/*\" { capabilities=[\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-cache-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-cache-test","token_ttl":"15m"})),
    )
    .await;
    let attrs = json!({
        "tenant_id": TENANT,
        "display_name": "WSF Cache Tenant",
        "compliance_scopes": ["hipaa"],
        "default_allowed_routes": ["cloud_allowed"],
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
async fn ring3_cache_syncs_real_bridge_material() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP ring3_cache_syncs_real_bridge_material: WSF_OPENBAO_ADDR unset (W5 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-cache-bridge").unwrap());
    let bridge = TrustBridge::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        bridge_signer.clone(),
        BridgeConfig::new("2026.07.03.cache", vec![4u8; 32]),
    );

    // Real cloud-cleared token, and a real bridge-signed revocation of some OTHER id.
    let token = bridge
        .issue_token(&IssueTokenRequest::new(
            TENANT,
            "clinician-1",
            vec!["clinician".to_string()],
        ))
        .await
        .expect("issue token");
    assert_eq!(
        token.allowed_routes,
        vec![Route::CloudAllowed],
        "tenant default is cloud_allowed"
    );

    let mut snap = RevocationSnapshot::new(
        "cache-snap-1",
        "2026-07-03T00:00:00Z",
        "2027-01-01T00:00:00Z",
    );
    snap.revoked_tokens.push("tok_some-other".to_string());
    let snap = bridge.sign_revocation(snap).expect("sign revocation");

    // Ring 2 -> Ring 3 sync with real material.
    let mut cache = Ring3Cache::new(bridge_signer.public_key().to_vec(), TTL);
    cache
        .refresh(snap, now_secs())
        .expect("refresh from real bridge material");

    // Fresh: cloud allowed.
    let fresh = cache.decide(&token, Utc::now());
    assert!(fresh.token_valid);
    assert_eq!(fresh.effective_routes, vec![Route::CloudAllowed]);

    // Air-gap: cloud narrowed to local-only.
    cache.set_air_gapped();
    let gapped = cache.decide(&token, Utc::now());
    assert_eq!(
        gapped.effective_routes,
        vec![Route::LocalOnly],
        "air-gap denies cloud"
    );
    cache.clear_air_gap();

    // A revoked real token is denied offline.
    let mut snap2 = RevocationSnapshot::new(
        "cache-snap-2",
        "2026-07-03T00:00:00Z",
        "2027-01-01T00:00:00Z",
    );
    snap2.revoked_tokens.push(token.token_id.clone());
    let snap2 = bridge.sign_revocation(snap2).expect("sign revocation 2");
    cache.refresh(snap2, now_secs()).unwrap();
    let revoked = cache.decide(&token, Utc::now());
    assert!(!revoked.token_valid, "revoked token denied offline");
    assert!(revoked.effective_routes.is_empty());

    eprintln!(
        "W5 live gate PASSED against {addr}: real token + revocation synced; cloud→local under air-gap; revoked denied offline"
    );
}
