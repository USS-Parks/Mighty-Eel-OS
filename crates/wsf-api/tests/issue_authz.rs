//! A3 gate — issuance authority is derived from the authenticated principal +
//! server-side tenant policy, never from the request body. Closes AF-002 with a
//! live black-box test against the real WSF issue handler + live OpenBao bridge.
//!
//! Env-gated on `WSF_OPENBAO_ADDR` (self-bootstraps AppRole + KV + tenant). The
//! router uses the explicit local-dev principal bound to `TENANT`, so the caller
//! is authenticated as `TENANT`; the assertions prove the caller cannot escape
//! that tenant, grant itself an ungranted role, or exceed the budget ceiling.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};

use base64::Engine;
use chrono::{Duration, Utc};
use fabric_contracts::Audience;
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use wsf_api::AppState;
use wsf_api::auth::{
    LocalDevAuthenticator, WorkloadAuthenticator, WorkloadCredential, mint_credential,
};
use wsf_api::policy::{StaticTenantPolicies, TenantPolicyStore};
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::Ledger;
use wsf_seal::{SealService, SealServiceConfig};

const ROLE: &str = "wsf-authz-test";
const TENANT: &str = "wsf-authz-tenant";
const OTHER_TENANT: &str = "wsf-authz-victim";
const TRANSIT_KEY: &str = "wsf-authz-dek";

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
    bao(
        c,
        addr,
        tok,
        Method::POST,
        "sys/auth/approle",
        Some(json!({"type":"approle"})),
    )
    .await;
    bao(
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
        "sys/policies/acl/wsf-authz-test",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-authz-test","token_ttl":"15m"})),
    )
    .await;
    // Both tenants exist in OpenBao, so a "wrong tenant" is a real, resolvable
    // tenant — the derivation, not a lookup miss, is what keeps the caller in
    // its own tenant.
    for t in [TENANT, OTHER_TENANT] {
        let attrs = json!({
            "tenant_id": t, "display_name": t,
            "compliance_scopes": ["hipaa"], "default_allowed_routes": ["local_only"],
            "max_data_classification": "restricted"
        });
        bao(
            c,
            addr,
            tok,
            Method::POST,
            &format!("kv/data/tenants/{t}"),
            Some(json!({ "data": { "attributes": attrs.to_string() } })),
        )
        .await;
    }
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
async fn issuance_authority_is_derived_not_caller_supplied() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP issue_authz: set WSF_OPENBAO_ADDR (A3 live gate)");
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

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-authz-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            ob(),
            bridge_signer,
            BridgeConfig::new("2026.07.authz", vec![7u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            ob(),
            Client::new(),
            BrokerConfig::new(
                "us-east-1",
                "http://127.0.0.1:5566",
                "kv/data/broker/aws-root",
            ),
        )),
        seal: Arc::new(SealService::new(
            ob(),
            Arc::new(RustCryptoMlDsa87::generate("wsf-authz-seal").unwrap()),
            SealServiceConfig {
                transit_key: TRANSIT_KEY.to_string(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-authz-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        // Caller is authenticated as TENANT and may be granted only `clinician`.
        auth: Arc::new(LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(StaticTenantPolicies::single_dev(TENANT, &["clinician"])),
        grants: Arc::new(wsf_api::grants::StaticGrants::new()),
    };

    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = Client::new();
    let issue = |body: Value| {
        let http = http.clone();
        let url = format!("{base}/v1/tokens/issue");
        async move { http.post(url).json(&body).send().await.expect("issue req") }
    };

    // 1. Valid bounded intent → 200, and the token is bound to the PRINCIPAL's
    //    tenant, with exactly the granted role.
    let ok = issue(json!({ "requested_roles": ["clinician"] })).await;
    assert_eq!(ok.status(), StatusCode::OK, "valid issuance succeeds");
    let token: Value = ok.json().await.expect("token json");
    assert_eq!(
        token["token"]["tenant_id"], TENANT,
        "issued token is bound to the authenticated principal's tenant"
    );
    assert_eq!(token["token"]["roles"], json!(["clinician"]));

    // 2. Smuggle a foreign tenant_id in the body → 422 (deny_unknown_fields).
    //    The caller literally cannot express a tenant on the wire.
    let smuggle =
        issue(json!({ "tenant_id": OTHER_TENANT, "requested_roles": ["clinician"] })).await;
    assert_eq!(
        smuggle.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a smuggled tenant_id is rejected, not honored"
    );

    // 3. Request a role the tenant policy does not grant → 403.
    let esc = issue(json!({ "requested_roles": ["admin"] })).await;
    assert_eq!(
        esc.status(),
        StatusCode::FORBIDDEN,
        "ungranted role is refused"
    );

    // 4. Request a budget above the tenant ceiling → 403 (no unlimited budget).
    let over = issue(json!({
        "requested_roles": ["clinician"],
        "budget": { "token_cap": 1_000_000_000_000u64 }
    }))
    .await;
    assert_eq!(
        over.status(),
        StatusCode::FORBIDDEN,
        "over-ceiling budget is refused"
    );

    // 5. Smuggle raw `roles` authority → 422.
    let raw_roles = issue(json!({ "roles": ["admin"] })).await;
    assert_eq!(raw_roles.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 6. A4 receipts: the successful issue and the two authz refusals above
    //    (ungranted role, over-ceiling budget) are all on the ledger.
    let receipts: Value = http
        .get(format!("{base}/v1/receipts"))
        .send()
        .await
        .expect("receipts req")
        .json()
        .await
        .expect("receipts json");
    let decisions: Vec<&Value> = receipts["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| &e["receipt"])
        .filter(|r| r["kind"] == "issuance_decision")
        .collect();
    assert!(
        decisions
            .iter()
            .any(|r| r["decision"] == "allow" && r["tenant_id"] == TENANT),
        "allow receipt for the successful issue"
    );
    assert!(
        decisions.iter().filter(|r| r["decision"] == "deny").count() >= 2,
        "deny receipts for the refused role + budget"
    );

    println!(
        "A3/A4 live gate PASSED against {addr}: authority server-derived; self-assignment refused; allow+deny receipted"
    );
}

/// A5 live gate — authenticated issuance against live OpenBao with **two
/// tenants and two workload identities**, using the real signed-credential
/// `WorkloadAuthenticator` (not the dev principal). Correct identity succeeds
/// and its token is bound to its own tenant; cross-tenant is structurally
/// impossible (the credential fixes the tenant) and role-escalation fails and
/// is receipted.
#[tokio::test]
async fn two_tenants_two_workloads_against_live_openbao() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP a5 two-tenant: set WSF_OPENBAO_ADDR (A5 live gate)");
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

    // The credential authority the ingress trusts.
    let authority = RustCryptoMlDsa87::generate("wsf-a5-authority").unwrap();
    let authenticator = WorkloadAuthenticator::new(
        Box::new(MlDsa87Verifier),
        authority.public_key().to_vec(),
        Audience::Wsf,
    );

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-a5-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    // Both tenants have a policy granting `clinician` (never `admin`).
    let policy = StaticTenantPolicies::single_dev(TENANT, &["clinician"]).with(
        StaticTenantPolicies::single_dev(OTHER_TENANT, &["clinician"])
            .policy_for(OTHER_TENANT)
            .unwrap(),
    );
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            ob(),
            bridge_signer,
            BridgeConfig::new("2026.07.a5", vec![8u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            ob(),
            Client::new(),
            BrokerConfig::new(
                "us-east-1",
                "http://127.0.0.1:5566",
                "kv/data/broker/aws-root",
            ),
        )),
        seal: Arc::new(SealService::new(
            ob(),
            Arc::new(RustCryptoMlDsa87::generate("wsf-a5-seal").unwrap()),
            SealServiceConfig {
                transit_key: TRANSIT_KEY.to_string(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-a5-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(authenticator),
        policy: Arc::new(policy),
        grants: Arc::new(wsf_api::grants::StaticGrants::new()),
    };
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let cred_header = |cred: &WorkloadCredential| {
        let json = serde_json::to_vec(cred).unwrap();
        format!(
            "Workload {}",
            base64::engine::general_purpose::STANDARD.encode(json)
        )
    };
    // Two distinct workload identities, one per tenant.
    let id1 = mint_credential(
        &authority,
        "spiffe://mai/wl-1",
        TENANT,
        "",
        None,
        Audience::Wsf,
        future.clone(),
    );
    let id2 = mint_credential(
        &authority,
        "spiffe://mai/wl-2",
        OTHER_TENANT,
        "",
        None,
        Audience::Wsf,
        future.clone(),
    );

    let http = Client::new();
    let issue_as = |cred: WorkloadCredential, body: Value| {
        let http = http.clone();
        let url = format!("{base}/v1/tokens/issue");
        let hdr = cred_header(&cred);
        async move {
            http.post(url)
                .header(reqwest::header::AUTHORIZATION, hdr)
                .json(&body)
                .send()
                .await
                .expect("issue req")
        }
    };

    // Identity 1 → token bound to TENANT.
    let r1 = issue_as(id1.clone(), json!({ "requested_roles": ["clinician"] })).await;
    assert_eq!(r1.status(), StatusCode::OK, "workload 1 issuance succeeds");
    let t1: Value = r1.json().await.unwrap();
    assert_eq!(
        t1["token"]["tenant_id"], TENANT,
        "token 1 bound to its own tenant"
    );

    // Identity 2 → token bound to OTHER_TENANT. Two identities, two tenants,
    // isolated: neither could ever name the other's tenant (the credential is
    // the only tenant source, and it is authority-signed).
    let r2 = issue_as(id2.clone(), json!({ "requested_roles": ["clinician"] })).await;
    assert_eq!(r2.status(), StatusCode::OK, "workload 2 issuance succeeds");
    let t2: Value = r2.json().await.unwrap();
    assert_eq!(
        t2["token"]["tenant_id"], OTHER_TENANT,
        "token 2 bound to its own tenant"
    );

    // Role-escalation by identity 1 (admin not grantable) → 403 + deny receipt.
    let esc = issue_as(id1.clone(), json!({ "requested_roles": ["admin"] })).await;
    assert_eq!(
        esc.status(),
        StatusCode::FORBIDDEN,
        "role escalation refused"
    );

    // No credential at all → 401 before the handler.
    let anon = http
        .post(format!("{base}/v1/tokens/issue"))
        .json(&json!({ "requested_roles": ["clinician"] }))
        .send()
        .await
        .expect("anon req");
    assert_eq!(
        anon.status(),
        StatusCode::UNAUTHORIZED,
        "unauthenticated issuance refused"
    );

    println!(
        "A5 live gate PASSED against {addr}: two workload identities, two tenants, isolated; escalation refused; anon 401"
    );
}
