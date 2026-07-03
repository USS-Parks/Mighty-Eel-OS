//! W1 gate — end-to-end token issue + verify against a **live** OpenBao.
//!
//! Env-gated (no `#[ignore]`, per plan §0.3.2): runs only when `WSF_OPENBAO_ADDR`
//! is set — a CI service container or a local
//! `docker run -e BAO_DEV_ROOT_TOKEN_ID=root -p 8200:8200 openbao/openbao`.
//! It self-bootstraps from the dev root token (enable AppRole + KV, create a
//! role, mint a secret_id, write a tenant), so it needs only a *bare* dev
//! OpenBao — not the full `deployment/openbao-staging/start-openbao.ps1`
//! provisioning. Without the env var it returns cleanly so the offline suite
//! stays green. `WSF_OPENBAO_TOKEN` overrides the root token (default `root`).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;
use std::time::Duration;

use fabric_contracts::{Budget, Classification, ComplianceScope, Route};
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{
    BridgeConfig, IssueTokenRequest, OpenBaoAuth, OpenBaoConfig, TrustBridge, verify_bundle,
};

const ROLE: &str = "wsf-bridge-test";
const TENANT: &str = "wsf-test-tenant";

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

/// Provision AppRole + KV + a role + tenant from the dev root token. Returns
/// `(role_id, secret_id)` for the bridge's AppRole login.
async fn provision(c: &Client, addr: &str, tok: &str) -> (String, String) {
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

    // Policy granting read on tenant records.
    let policy = "path \"kv/data/tenants/*\" { capabilities = [\"read\"] }";
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-bridge-test",
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
        Some(json!({"token_policies":"default,wsf-bridge-test","token_ttl":"15m"})),
    )
    .await;

    // role_id + a fresh secret_id.
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

    // Tenant record — attributes as a JSON string, matching start-openbao.ps1.
    let attrs = json!({
        "tenant_id": TENANT,
        "display_name": "WSF Test Tenant",
        "compliance_scopes": ["hipaa", "ocap"],
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

    (role_id, secret_id)
}

#[tokio::test]
async fn issue_and_verify_token_end_to_end() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP issue_and_verify_token_end_to_end: WSF_OPENBAO_ADDR unset (W1 live gate)");
        return;
    };

    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = provision(&c, &addr, &root_token()).await;

    let openbao = OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap();
    assert!(
        openbao.health().await,
        "openbao should be reachable + unsealed"
    );

    let signer = Arc::new(RustCryptoMlDsa87::generate("wsf-bridge-test-key").unwrap());
    let cfg = BridgeConfig::new("2026.07.03.test", vec![9u8; 32]).with_locale("US", "us_person");
    let bridge = TrustBridge::new(openbao, signer, cfg);

    let req = IssueTokenRequest::new(TENANT, "clinician-42", vec!["clinician".to_string()])
        .with_budget(Budget {
            token_cap: 100_000,
            usd_cap_cents: 5_000,
            tool_call_cap: 50,
            ..Default::default()
        });
    let token = bridge
        .issue_token(&req)
        .await
        .expect("issue token against live openbao");

    // Envelope mapped from the live tenant record.
    assert_eq!(token.tenant_id, TENANT);
    assert_eq!(
        token.compliance_scopes,
        vec![ComplianceScope::Hipaa, ComplianceScope::Ocap]
    );
    assert_eq!(token.allowed_routes, vec![Route::LocalOnly]);
    assert_eq!(token.max_data_classification, Classification::Restricted);
    assert!(token.subject_id.is_none(), "subject must be pseudonymous");
    assert!(token.budget.is_some());

    // Off-host verify: public key only, no OpenBao round-trip.
    fabric_token::verify(&token, &MlDsa87Verifier, bridge.public_key())
        .expect("token verifies off-host");

    // Tamper → verification fails.
    let mut tampered = token.clone();
    tampered.max_data_classification = Classification::Secret;
    assert!(
        fabric_token::verify(&tampered, &MlDsa87Verifier, bridge.public_key()).is_err(),
        "tampered token must not verify"
    );

    // Bundle signing verifies off-host too (the other half of the W1 gate).
    let sig = bridge.sign_bundle(b"signed-policy-bundle-v1").unwrap();
    assert!(verify_bundle(
        b"signed-policy-bundle-v1",
        &sig,
        &MlDsa87Verifier,
        bridge.public_key()
    ));
    assert!(!verify_bundle(
        b"mutated-bundle",
        &sig,
        &MlDsa87Verifier,
        bridge.public_key()
    ));

    eprintln!("W1 live gate PASSED against {addr}");
}
