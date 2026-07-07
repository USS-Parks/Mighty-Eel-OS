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

use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use wsf_api::AppState;
use wsf_api::auth::LocalDevAuthenticator;
use wsf_api::policy::StaticTenantPolicies;
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

    println!(
        "A3 live gate PASSED against {addr}: tenant/subject/role/budget are server-derived; self-assignment refused"
    );
}
