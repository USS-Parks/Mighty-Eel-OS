//! O7 gate — draining a cordoned node **revokes each drained replica's runtime
//! token** against a live OpenBao (A3.2 node-lifecycle). The estate-side budget
//! behaviour is covered by the mock `maintenance.rs` test; this proves the
//! trust-sensitive half: a replica evicted for maintenance cannot keep acting on
//! a token that outlived its placement. Ring preservation on re-placement is the
//! S3 filter (unchanged) plus the O7 cordon exclusion, proven structurally.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, CORDON_LABEL, Controller, EstateClient, MaintenanceController, Reconciler,
    SyncStats,
};
use aog_estate::{
    AttestationProfile, Capacity, Kind, Node, NodeSpec, PlacementSpec, Resource, ResourceObject,
};
use fabric_contracts::Classification;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-o7-maint";
const PREFIX: &str = "kv/data/loom-maint";

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
path "kv/data/loom-maint/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-maint/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-o7-maint",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-o7-maint","token_ttl":"15m"})),
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
    let mut now = Instant::now();
    for _ in 0..30 {
        let s = c.sync(now).await.unwrap();
        if quiet(s) && idle(c) {
            return;
        }
        now += Duration::from_secs(6);
    }
    panic!("controller did not settle");
}

#[tokio::test]
async fn draining_a_cordoned_node_revokes_the_replicas_token() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP draining_a_cordoned_node_revokes_the_replicas_token: WSF_OPENBAO_ADDR unset (O7 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    let anchor = RustCryptoMlDsa87::generate("loom-o7-anchor").unwrap();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-o7-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let openbao =
        Arc::new(OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap());

    // A cordoned node hosting one placement, whose runtime token lives in OpenBao.
    let mut node = Node::new(
        "node-a",
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: Capacity::default(),
        },
    );
    node.metadata
        .labels
        .insert(CORDON_LABEL.to_owned(), "true".to_owned());
    client
        .ensure_created(ResourceObject::Node(node))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::Placement(Resource::new(
            "gw-r0",
            PlacementSpec {
                workload: "gw".to_owned(),
                node: "node-a".to_owned(),
                token_id: "rt:gw-r0".to_owned(),
            },
        )))
        .await
        .unwrap();

    let vault = openbao.login().await.unwrap();
    let token_path = format!("{PREFIX}/gw-r0");
    openbao
        .put_kv_data(&vault, &token_path, json!({ "token": "rt:gw-r0" }))
        .await
        .unwrap();
    assert!(
        openbao.get_kv_data(&vault, &token_path).await.is_ok(),
        "the replica's token is live before the drain"
    );

    // Drain the cordoned node with token revocation enabled.
    let reconciler = MaintenanceController::new(client.clone(), 1)
        .with_token_revocation(Arc::clone(&openbao), PREFIX);
    let mut controller = Controller::new(
        "maintenance",
        state.informer("Node/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // The placement is gone from the estate and its token is no longer fetchable.
    let remaining = client
        .list(Kind::Placement)
        .await
        .unwrap()
        .into_iter()
        .filter(|o| matches!(o, ResourceObject::Placement(p) if p.spec.node == "node-a"))
        .count();
    assert_eq!(remaining, 0, "the cordoned node is drained");
    let after = openbao.get_kv_data(&vault, &token_path).await;
    let token_gone = match after {
        Err(_) => true,
        Ok(v) => v.get("token").is_none(),
    };
    assert!(
        token_gone,
        "the drained replica's runtime token is revoked from OpenBao"
    );
}
