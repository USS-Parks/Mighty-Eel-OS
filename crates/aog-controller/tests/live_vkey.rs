//! "a key change is reflected at the gateway without restart",
//! against a **live** OpenBao and the **real** `aog-gateway` (A3.2
//! no-mock-only). The controller writes the gateway's key-resolution entries;
//! the same `Gateway` instance resolves before and after the change — no
//! rebuild, no restart. Deleting the key retracts its entry (fail-closed): the
//! gateway then refuses it.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, Reconciler, SyncStats, VirtualKeyController,
};
use aog_estate::{
    CapabilitySpec, Kind, Phase, Resource, ResourceObject, VirtualKey, VirtualKeySpec,
};
use aog_gateway::{Gateway, GatewayConfig, GatewayError};
use chrono::Utc;
use fabric_contracts::{Budget, Classification, Route};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-r8-vk";
const PREFIX: &str = "kv/data/loom-vk";
const KEY: &str = "team-alpha-key";
const TENANT: &str = "acme";

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

async fn bootstrap(c: &Client, addr: &str, tok: &str) -> (String, String) {
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

    let policy = r#"
path "kv/data/loom-vk/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-vk/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r8-vk",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r8-vk","token_ttl":"15m"})),
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
    let role_id = rid["data"]["role_id"].as_str().expect("role_id").to_owned();
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
        .to_owned();
    (role_id, secret_id)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn capability(token_cap: u64, models: &[&str], class: Classification) -> CapabilitySpec {
    CapabilitySpec {
        budget: Budget {
            token_cap,
            ..Budget::default()
        },
        caveats: vec![],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: models.iter().map(|m| (*m).to_owned()).collect(),
        max_classification: class,
        ttl_seconds: 3600,
    }
}

fn quiet(stats: SyncStats) -> bool {
    stats.enqueued == 0 && stats.drained == 0 && stats.processed == 0
}

fn idle<R: Reconciler>(c: &Controller<R>) -> bool {
    c.queue_len() == 0 && c.delayed_len() == 0
}

async fn settle<R: Reconciler>(c: &mut Controller<R>) {
    for _ in 0..200 {
        let now = Instant::now();
        let s = c.sync(now).await.unwrap();
        if quiet(s) && idle(c) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("controller did not settle within 200 rounds");
}

async fn get_vkey(client: &EstateClient) -> VirtualKey {
    let Some(ResourceObject::VirtualKey(v)) = client.get(Kind::VirtualKey, KEY).await.unwrap()
    else {
        panic!("vkey missing");
    };
    v
}

#[tokio::test]
async fn a_virtual_key_change_is_reflected_at_the_gateway_without_restart() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP a_virtual_key_change_is_reflected_at_the_gateway_without_restart: WSF_OPENBAO_ADDR unset (live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-r8-anchor").unwrap());
    let signer: Arc<dyn Signer> = anchor.clone();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r8-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let openbao = Arc::new(
        OpenBaoAuth::new(OpenBaoConfig::new(
            &addr,
            role_id.clone(),
            secret_id.clone(),
        ))
        .unwrap(),
    );

    // The real gateway, pointed at the same OpenBao + prefix, verifying against
    // the same anchor. Built ONCE — never rebuilt or restarted below.
    let gateway = Gateway::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: PREFIX.to_owned(),
        },
    );

    // cap-basic (1000-token cap, one model, Internal) and a key resolving to it.
    client
        .ensure_created(ResourceObject::Capability(Resource::new(
            "cap-basic",
            capability(1000, &["gpt-x"], Classification::Internal),
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::Capability(Resource::new(
            "cap-premium",
            capability(100_000, &["gpt-x", "gpt-pro"], Classification::Restricted),
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::VirtualKey(Resource::new(
            KEY,
            VirtualKeySpec {
                tenant: TENANT.to_owned(),
                capability: "cap-basic".to_owned(),
                display_name: "Team Alpha".to_owned(),
            },
        )))
        .await
        .unwrap();

    let reconciler =
        VirtualKeyController::new(client.clone(), Arc::clone(&openbao), PREFIX, signer);
    let mut controller = Controller::new(
        "virtualkey",
        state.informer("VirtualKey/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // ── Resolves to cap-basic's scope + budget at the gateway.
    let vkey = get_vkey(&client).await;
    assert_eq!(vkey.status.as_ref().expect("status").phase, Phase::Ready);
    assert_eq!(
        vkey.status.expect("status").resolved_token.as_deref(),
        Some("vk:acme:team-alpha-key")
    );
    let ctx = gateway
        .resolve_and_check(KEY, Utc::now())
        .await
        .expect("key resolves at the gateway");
    assert_eq!(ctx.tenant_id, TENANT);
    assert_eq!(ctx.token.budget.as_ref().expect("budget").token_cap, 1000);
    assert_eq!(ctx.token.allowed_models, vec!["gpt-x".to_owned()]);
    assert_eq!(ctx.token.max_data_classification, Classification::Internal);

    // ── Repoint the key at cap-premium. No gateway restart.
    let mut updated = get_vkey(&client).await;
    updated.spec.capability = "cap-premium".to_owned();
    client
        .update(ResourceObject::VirtualKey(updated))
        .await
        .unwrap();
    settle(&mut controller).await;

    let ctx = gateway
        .resolve_and_check(KEY, Utc::now())
        .await
        .expect("key still resolves after the change");
    assert_eq!(
        ctx.token.budget.as_ref().expect("budget").token_cap,
        100_000,
        "the new capability's budget is reflected without a restart"
    );
    assert!(
        ctx.token.allowed_models.contains(&"gpt-pro".to_owned()),
        "the new capability's model set is reflected"
    );
    assert_eq!(
        ctx.token.max_data_classification,
        Classification::Restricted
    );

    // ── Delete the key: its entry is retracted, and the gateway refuses it.
    client.delete(Kind::VirtualKey, KEY).await.unwrap();
    settle(&mut controller).await;
    assert!(
        client.get(Kind::VirtualKey, KEY).await.unwrap().is_none(),
        "the key object is gone (finalizer released after retraction)"
    );
    assert!(
        matches!(
            gateway.resolve_and_check(KEY, Utc::now()).await,
            Err(GatewayError::UnknownKey)
        ),
        "a deleted key no longer resolves at the gateway"
    );
}
