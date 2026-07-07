//! A4 gate (offline) — the issuance permission matrix + allow/deny receipts.
//!
//! Denials are decided *before* the bridge is ever dialed, so this runs with no
//! OpenBao: the bridge/broker/seal are constructed with unused config and never
//! contacted. Each case asserts the HTTP status **and** that a matching deny
//! receipt landed on the ledger. The allow-path receipt is covered live in
//! `issue_authz.rs`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use fabric_contracts::{Budget, Classification};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use wsf_api::AppState;
use wsf_api::auth::LocalDevAuthenticator;
use wsf_api::policy::{IssuanceMode, StaticTenantPolicies, TenantIssuancePolicy};
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{SealService, SealServiceConfig};

const TENANT: &str = "a4-tenant";

fn modes(ms: &[IssuanceMode]) -> BTreeSet<IssuanceMode> {
    ms.iter().copied().collect()
}

fn roles(rs: &[&str]) -> BTreeSet<String> {
    rs.iter().map(ToString::to_string).collect()
}

/// Build a router whose tenant policy is exactly `policy`, authenticated as a
/// human dev principal in `TENANT` (⇒ self-service unless an admin role is asked).
async fn spawn(policy: TenantIssuancePolicy) -> String {
    let unused = || OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap();
    let signer = Arc::new(RustCryptoMlDsa87::generate("a4-bridge").unwrap());
    let anchor = signer.public_key().to_vec();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            unused(),
            signer,
            BridgeConfig::new("a4", vec![9u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            unused(),
            Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/x"),
        )),
        seal: Arc::new(SealService::new(
            unused(),
            Arc::new(RustCryptoMlDsa87::generate("a4-seal").unwrap()),
            SealServiceConfig {
                transit_key: "x".into(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("a4-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(StaticTenantPolicies::new().with(policy)),
        grants: Arc::new(wsf_api::grants::StaticGrants::new()),
        auditors: Arc::new(wsf_api::audit::StaticAuditors::none()),
    };
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn deny_receipts(base: &str) -> Vec<Value> {
    let http = Client::new();
    let resp: Value = http
        .get(format!("{base}/v1/receipts"))
        .send()
        .await
        .expect("receipts req")
        .json()
        .await
        .expect("receipts json");
    resp["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e["receipt"].clone())
        .filter(|r| r["kind"] == "issuance_decision" && r["decision"] == "deny")
        .collect()
}

fn base_policy(permitted: &[IssuanceMode], depth: u32) -> TenantIssuancePolicy {
    TenantIssuancePolicy {
        tenant_id: TENANT.into(),
        grantable_roles: roles(&["user", "clinician", "admin"]),
        admin_roles: roles(&["admin"]),
        permitted_modes: modes(permitted),
        max_delegation_depth: depth,
        max_classification: Classification::Restricted,
        max_budget: Budget {
            token_cap: 1000,
            usd_cap_cents: 500,
            tool_call_cap: 10,
            ..Budget::default()
        },
        allowed_models: Vec::new(),
    }
}

async fn post_issue(base: &str, body: Value) -> StatusCode {
    Client::new()
        .post(format!("{base}/v1/tokens/issue"))
        .json(&body)
        .send()
        .await
        .expect("issue req")
        .status()
}

#[tokio::test]
async fn self_service_mode_not_permitted_is_denied_and_receipted() {
    // Tenant permits only service-to-service; a human self-service request fails.
    let base = spawn(base_policy(&[IssuanceMode::ServiceToService], 3)).await;
    let status = post_issue(&base, json!({ "requested_roles": ["user"] })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let denies = deny_receipts(&base).await;
    assert_eq!(denies.len(), 1, "exactly one deny receipt");
    assert_eq!(denies[0]["mode"], "self_service");
    assert_eq!(
        denies[0]["reason"],
        "issuance mode is not permitted for this tenant"
    );
    assert_eq!(denies[0]["tenant_id"], TENANT);
}

#[tokio::test]
async fn administrative_delegation_forbidden_at_zero_depth_is_denied() {
    // Admin mode permitted, but the tenant forbids delegation (depth 0).
    let base = spawn(base_policy(
        &[IssuanceMode::SelfService, IssuanceMode::Administrative],
        0,
    ))
    .await;
    let status = post_issue(&base, json!({ "requested_roles": ["admin"] })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let denies = deny_receipts(&base).await;
    assert_eq!(denies.len(), 1);
    assert_eq!(denies[0]["mode"], "administrative");
    assert_eq!(
        denies[0]["reason"],
        "tenant forbids delegation for this issuance mode"
    );
}

#[tokio::test]
async fn ungranted_role_is_denied_with_mode_labeled_receipt() {
    // Self-service permitted; the requested role is simply not grantable.
    let base = spawn(base_policy(&[IssuanceMode::SelfService], 3)).await;
    // "auditor" is not in grantable_roles.
    let status = post_issue(&base, json!({ "requested_roles": ["auditor"] })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let denies = deny_receipts(&base).await;
    assert_eq!(denies.len(), 1);
    assert_eq!(denies[0]["mode"], "self_service");
    assert_eq!(
        denies[0]["reason"],
        "requested role is not grantable for this tenant"
    );
}

#[tokio::test]
async fn over_ceiling_budget_is_denied_and_receipted() {
    let base = spawn(base_policy(&[IssuanceMode::SelfService], 3)).await;
    let status = post_issue(
        &base,
        json!({ "requested_roles": ["user"], "budget": { "token_cap": 999_999u64 } }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let denies = deny_receipts(&base).await;
    assert_eq!(denies.len(), 1);
    assert_eq!(
        denies[0]["reason"],
        "requested budget exceeds tenant ceiling"
    );
}

#[tokio::test]
async fn unknown_tenant_is_denied_before_any_policy() {
    // Router authenticated as TENANT, but the store has a *different* tenant.
    let other = base_policy(&[IssuanceMode::SelfService], 3);
    let mut policy = other;
    policy.tenant_id = "someone-else".into();
    let base = spawn(policy).await;
    let status = post_issue(&base, json!({ "requested_roles": ["user"] })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let denies = deny_receipts(&base).await;
    assert_eq!(denies.len(), 1);
    assert_eq!(denies[0]["reason"], "no issuance policy for this tenant");
    assert_eq!(denies[0]["mode"], "unknown");
}
