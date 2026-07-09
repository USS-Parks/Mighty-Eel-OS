//! "an intent → the token is denied on every replica AND on an
//! air-gapped node via media", against a **live** OpenBao and the **real**
//! `aog-gateway` kill switch (A3.2 no-mock-only).
//!
//! A virtual key is provisioned so the gateway resolves it to a token;
//! declaring a `RevocationIntent` for that token makes the controller publish
//! a signed snapshot to the online kill-switch path AND to a removable-media
//! file. The same gateway then refuses the key (`Revoked`), and the media
//! snapshot — verified offline with the public key alone — reports the token
//! revoked.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, Reconciler, RevocationController, SyncStats,
    VirtualKeyController,
};
use aog_estate::{
    CapabilitySpec, Kind, Resource, ResourceObject, RevocationIntentSpec, RevocationTarget,
    VirtualKeySpec,
};
use aog_gateway::{Gateway, GatewayConfig, GatewayError};
use chrono::Utc;
use fabric_contracts::{Budget, Classification, Route};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use fabric_revocation::RevocationSnapshot;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-r9-rev";
const VK_PREFIX: &str = "kv/data/loom-r9-vk";
const REV_PATH: &str = "kv/data/loom-r9-rev/estate";
const KEY: &str = "kill-key";
const TENANT: &str = "acme";
const TOKEN_ID: &str = "vk:acme:kill-key";

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
path "kv/data/loom-r9-vk/*"       { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-r9-vk/*"   { capabilities = ["read", "delete", "list"] }
path "kv/data/loom-r9-rev/*"      { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-r9-rev/*"  { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r9-rev",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r9-rev","token_ttl":"15m"})),
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

#[tokio::test]
async fn an_intent_denies_a_token_on_every_replica_and_over_media() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP an_intent_denies_a_token_on_every_replica_and_over_media: WSF_OPENBAO_ADDR unset (live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;
    // Pristine kill-switch path: a stale snapshot from a prior run (signed by a
    // different anchor) would fail the gateway closed before we revoke anything.
    bao(
        &http,
        &addr,
        &root_token(),
        Method::DELETE,
        "kv/metadata/loom-r9-rev/estate",
        None,
    )
    .await;

    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-r9-anchor").unwrap());
    let signer: Arc<dyn Signer> = anchor.clone();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r9-live"),
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

    // The real gateway: resolves virtual keys, and its kill switch reads REV_PATH.
    let gateway = Gateway::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        GatewayConfig {
            token_public_key: anchor.public_key().to_vec(),
            virtual_key_kv_prefix: VK_PREFIX.to_owned(),
        },
    )
    .with_revocation_path(REV_PATH);

    // Provision a resolvable key: a capability + a virtual key → a token.
    client
        .ensure_created(ResourceObject::Capability(Resource::new(
            "cap",
            CapabilitySpec {
                budget: Budget {
                    token_cap: 10_000,
                    ..Budget::default()
                },
                caveats: vec![],
                allowed_routes: vec![Route::LocalOnly],
                allowed_models: vec!["gpt-x".to_owned()],
                max_classification: Classification::Internal,
                ttl_seconds: 3600,
            },
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::VirtualKey(Resource::new(
            KEY,
            VirtualKeySpec {
                tenant: TENANT.to_owned(),
                capability: "cap".to_owned(),
                display_name: "Kill Me".to_owned(),
            },
        )))
        .await
        .unwrap();
    let mut vkeys = Controller::new(
        "virtualkey",
        state.informer("VirtualKey/"),
        VirtualKeyController::new(
            client.clone(),
            Arc::clone(&openbao),
            VK_PREFIX,
            signer.clone(),
        ),
        Arc::new(AlwaysLeader),
    );
    settle(&mut vkeys).await;

    // ── Before revocation: the gateway resolves the key.
    let ctx = gateway
        .resolve_and_check(KEY, Utc::now())
        .await
        .expect("key resolves before revocation");
    assert_eq!(ctx.token.token_id, TOKEN_ID);

    // ── Declare the kill.
    client
        .ensure_created(ResourceObject::RevocationIntent(Resource::new(
            "kill-1",
            RevocationIntentSpec {
                target: RevocationTarget::Token(TOKEN_ID.to_owned()),
                reason: "compromised key".to_owned(),
            },
        )))
        .await
        .unwrap();

    let media = fresh_dir("loom-r9-media");
    let mut revs = Controller::new(
        "revocation",
        state.informer("RevocationIntent/"),
        RevocationController::new(client.clone(), Arc::clone(&openbao), signer, REV_PATH)
            .with_media_dir(media.clone()),
        Arc::new(AlwaysLeader),
    );
    settle(&mut revs).await;

    // ── On every replica: the real gateway now refuses the key.
    assert!(
        matches!(
            gateway.resolve_and_check(KEY, Utc::now()).await,
            Err(GatewayError::Revoked)
        ),
        "the revoked token is denied at the gateway"
    );

    // ── Over media: an air-gapped node imports the snapshot, verifies it
    //    offline with the public key alone, and sees the token revoked.
    let bytes =
        std::fs::read(media.join("estate-revocation.json")).expect("media snapshot written");
    let snapshot: RevocationSnapshot =
        serde_json::from_slice(&bytes).expect("media snapshot parses");
    fabric_revocation::verify(&snapshot, &MlDsa87Verifier, anchor.public_key())
        .expect("media snapshot verifies offline with the public key alone");
    assert!(
        snapshot.is_token_revoked(TOKEN_ID),
        "the media snapshot revokes the token — the air-gap kill"
    );

    // ── The intent is acknowledged as propagated, not merely asserted.
    let Some(ResourceObject::RevocationIntent(intent)) =
        client.get(Kind::RevocationIntent, "kill-1").await.unwrap()
    else {
        panic!("intent missing");
    };
    assert!(intent.status.expect("intent status").propagated);
}
