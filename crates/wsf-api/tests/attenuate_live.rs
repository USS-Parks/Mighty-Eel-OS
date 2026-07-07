//! T7 live gate — issue → attenuate → verify plus the AF-001 adversarial cases,
//! black-box against the real WSF API + live OpenBao bridge.
//!
//! Env-gated on `WSF_OPENBAO_ADDR`. Proves the fix end-to-end over HTTP, not
//! only at the unit level: a forged/tampered parent is refused (403), a widening
//! restriction is refused (422), and a valid narrowing yields a child whose
//! identity is inherited from the authenticated parent.
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

const ROLE: &str = "wsf-atten-test";
const TENANT: &str = "wsf-atten-tenant";

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
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/wsf-atten-test",
        Some(json!({ "policy": "path \"kv/data/tenants/*\" { capabilities=[\"read\"] }" })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,wsf-atten-test","token_ttl":"15m"})),
    )
    .await;
    let attrs = json!({
        "tenant_id": TENANT, "display_name": TENANT,
        "compliance_scopes": ["hipaa"], "default_allowed_routes": ["local_only"],
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
    .expect("role-id");
    let role_id = rid["data"]["role_id"].as_str().unwrap().to_string();
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
    .expect("secret-id");
    let secret_id = sid["data"]["secret_id"].as_str().unwrap().to_string();
    (role_id, secret_id)
}

#[tokio::test]
async fn issue_attenuate_verify_and_adversarial_parents() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP attenuate_live: set WSF_OPENBAO_ADDR (T7 live gate)");
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

    let bridge_signer = Arc::new(RustCryptoMlDsa87::generate("wsf-atten-bridge").unwrap());
    let anchor = bridge_signer.public_key().to_vec();
    // Keep a handle to mint an anchor-signed *legacy* parent (T6) below.
    let anchor_signer = bridge_signer.clone();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            ob(),
            bridge_signer,
            BridgeConfig::new("2026.07.atten", vec![6u8; 32]),
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
            Arc::new(RustCryptoMlDsa87::generate("wsf-atten-seal").unwrap()),
            SealServiceConfig {
                transit_key: "wsf-atten-dek".into(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger: Arc::new(Mutex::new(Ledger::new(Arc::new(
            RustCryptoMlDsa87::generate("wsf-atten-ledger").unwrap(),
        )))),
        token_public_key: Arc::new(anchor),
        auth: Arc::new(LocalDevAuthenticator::for_wsf(TENANT)),
        policy: Arc::new(StaticTenantPolicies::single_dev(TENANT, &["clinician"])),
    };
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = Client::new();

    // Issue a real parent token via the live handler.
    let issued: Value = http
        .post(format!("{base}/v1/tokens/issue"))
        .json(&json!({ "requested_roles": ["clinician"] }))
        .send()
        .await
        .expect("issue")
        .json()
        .await
        .expect("issue json");
    let parent = issued["token"].clone();
    assert_eq!(parent["tenant_id"], TENANT);

    let attenuate = |body: Value| {
        let http = http.clone();
        let url = format!("{base}/v1/tokens/attenuate");
        async move {
            http.post(url)
                .json(&body)
                .send()
                .await
                .expect("attenuate req")
        }
    };

    // 1. Valid narrowing → 200; child inherits the parent's tenant/subject.
    let ok = attenuate(json!({
        "parent": parent,
        "restrictions": { "new_token_id": "child-1", "allowed_routes": [] }
    }))
    .await;
    assert_eq!(ok.status(), StatusCode::OK, "valid attenuation succeeds");
    let child: Value = ok.json().await.unwrap();
    assert_eq!(child["token"]["tenant_id"], TENANT, "child inherits tenant");
    assert_eq!(
        child["token"]["attenuation"]["parent_id"],
        parent["token_id"]
    );

    // 2. Verify the child end-to-end.
    let ver: Value = http
        .post(format!("{base}/v1/tokens/verify"))
        .json(&json!({ "token": child["token"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ver["valid"], true, "attenuated child verifies");

    // 3. Tampered parent (mutate a field after signing) → 403: lineage can't be forged.
    let mut tampered = parent.clone();
    tampered["roles"] = json!(["clinician", "admin"]);
    let t = attenuate(json!({
        "parent": tampered,
        "restrictions": { "new_token_id": "child-2" }
    }))
    .await;
    assert_eq!(
        t.status(),
        StatusCode::FORBIDDEN,
        "tampered parent is refused"
    );

    // 4. Forged parent signed by an attacker key (not the anchor) → 403.
    let attacker = RustCryptoMlDsa87::generate("attacker").unwrap();
    let mut forged: fabric_contracts::TrustToken = serde_json::from_value(parent.clone()).unwrap();
    forged.token_id = "forged".into();
    let forged = fabric_token::issue(forged, &attacker).unwrap();
    let f = attenuate(json!({
        "parent": forged,
        "restrictions": { "new_token_id": "child-3" }
    }))
    .await;
    assert_eq!(
        f.status(),
        StatusCode::FORBIDDEN,
        "attacker-signed parent is refused"
    );

    // 5. Widening restriction (role the parent lacks) → 422.
    let w = attenuate(json!({
        "parent": parent,
        "restrictions": { "new_token_id": "child-4", "roles": ["admin"] }
    }))
    .await;
    assert_eq!(
        w.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "widening is refused"
    );

    // 6. T6: a validly anchor-signed *legacy* parent (bundle != the bridge's
    //    current) may not be attenuated — no v1 attenuation.
    let mut legacy: fabric_contracts::TrustToken = serde_json::from_value(parent.clone()).unwrap();
    legacy.token_id = "legacy-parent".into();
    legacy.trust_bundle_version = "2020.legacy.v1".into();
    let legacy = fabric_token::issue(legacy, anchor_signer.as_ref()).unwrap();
    let l = attenuate(json!({
        "parent": legacy,
        "restrictions": { "new_token_id": "child-5" }
    }))
    .await;
    // The handler denies legacy by default (no migration flag): the version
    // policy in verify_in_context refuses it (403) before the child is built.
    assert_eq!(
        l.status(),
        StatusCode::FORBIDDEN,
        "legacy (v1) parent is refused (T6 deny-by-default)"
    );

    println!(
        "T7 live gate PASSED against {addr}: issue→attenuate→verify; forged/tampered parent 403; widening 422; legacy parent 403 (T6)"
    );
}
