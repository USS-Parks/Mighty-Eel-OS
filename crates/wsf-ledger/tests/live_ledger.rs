//! W4 gate — multi-service receipts chain correctly + the exported evidence pack
//! verifies off-host. Env-gated on `WSF_OPENBAO_ADDR`.
//!
//! Produces **real** receipts against live OpenBao: a `wsf-bridge` token issuance
//! (→ audit correlation) and `wsf-seal` seal + unseal ops (→ seal receipts,
//! whose data keys are really Transit-wrapped), ingests them from two sources
//! into one ledger, verifies the chain, correlation-queries by token id, exports
//! a signed pack, and verifies it with the public key alone.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use chrono::Utc;
use fabric_contracts::Classification;
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use wsf_bridge::{BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_ledger::{Ledger, verify_pack};
use wsf_seal::{LabelSpec, SealRequest, SealService, SealServiceConfig, UnsealRequest};

const ROLE: &str = "wsf-ledger-test";
const TENANT: &str = "wsf-ledger-tenant";
const TRANSIT_KEY: &str = "wsf-ledger-dek";

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

/// Provision approle + kv (tenant) + transit (key), and a role whose policy
/// grants kv-read on tenants and transit encrypt/decrypt.
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
        "path \"kv/data/tenants/*\" {{ capabilities=[\"read\"] }}\npath \"transit/encrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}\npath \"transit/decrypt/{TRANSIT_KEY}\" {{ capabilities=[\"update\"] }}"
    );
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-ledger-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-ledger-test","token_ttl":"15m"})),
    )
    .await;

    let attrs = json!({
        "tenant_id": TENANT,
        "display_name": "WSF Ledger Tenant",
        "compliance_scopes": ["hipaa","ocap"],
        "default_allowed_routes": ["local_only"],
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
async fn multi_service_ledger_pack_against_live_openbao() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP multi_service_ledger_pack_against_live_openbao: WSF_OPENBAO_ADDR unset (W4 live gate)"
        );
        return;
    };

    let c = Client::new();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    // wsf-bridge: issue a token, capture its audit correlation.
    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-ledger-bridge").unwrap());
    let bridge = TrustBridge::new(
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .unwrap(),
        bridge_signer.clone(),
        BridgeConfig::new("2026.07.03.led", vec![3u8; 32]),
    );
    let token = bridge
        .issue_token(&IssueTokenRequest::new(
            TENANT,
            "clinician-1",
            vec!["clinician".to_string()],
        ))
        .await
        .expect("issue token");
    let correlation = bridge.audit_correlation(&token);

    // wsf-seal: seal + unseal (both allow), producing Transit-wrapped receipts.
    let seal_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-ledger-seal").unwrap());
    let seal = SealService::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        seal_signer,
        SealServiceConfig {
            transit_key: TRANSIT_KEY.to_string(),
            token_public_key: bridge_signer.public_key().to_vec(),
        },
    );
    let now = Utc::now();
    let envelope = seal
        .seal(
            SealRequest {
                token: token.clone(),
                plaintext: b"protected health information".to_vec(),
                label: LabelSpec {
                    classification: Classification::Restricted,
                    compliance_scopes: vec![],
                    origin: "ingest".to_string(),
                    permitted_ops: vec!["unseal".to_string()],
                    permitted_destinations: vec![],
                    detected_entities: vec![],
                },
                envelope_id: "env-ledger".to_string(),
            },
            now,
        )
        .await
        .expect("seal");
    let _plaintext = seal
        .unseal(
            UnsealRequest {
                token: token.clone(),
                envelope,
            },
            now,
        )
        .await
        .expect("unseal");
    let seal_receipts = seal.receipts_snapshot();
    assert_eq!(seal_receipts.len(), 2, "seal + unseal receipts");

    // Ingest into one ledger from two sources.
    let ledger_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-ledger-key").unwrap());
    let mut ledger = Ledger::new(ledger_signer);
    ledger
        .ingest("wsf-bridge", serde_json::to_value(&correlation).unwrap())
        .unwrap();
    for r in &seal_receipts {
        ledger
            .ingest("wsf-seal", serde_json::to_value(r).unwrap())
            .unwrap();
    }
    assert_eq!(ledger.len(), 3);

    // The cross-service chain verifies.
    ledger.verify().expect("multi-service chain verifies");

    // Correlation query joins the bridge + seal receipts by token id.
    let by_token = ledger.query("token_id", &token.token_id);
    assert_eq!(by_token.len(), 3, "all three receipts carry the token id");

    // Exported evidence pack verifies with the public key alone.
    let pack = ledger.export_pack(now.to_rfc3339()).unwrap();
    assert!(
        verify_pack(&pack, &MlDsa87Verifier, ledger.public_key()),
        "pack verifies off-host"
    );
    assert_eq!(pack.count, 3);

    eprintln!(
        "W4 live gate PASSED against {addr}: {}-entry cross-service ledger, pack head {} verifies off-host",
        pack.count, pack.head_hash
    );
}
