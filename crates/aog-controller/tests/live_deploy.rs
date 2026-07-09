//! O1 gate — replica-set convergence against a **live** OpenBao (A3.2
//! no-mock-only). Two proofs the pure planner (unit-tested in `deploy.rs`)
//! cannot give on its own, because they are about the OpenBao side effects:
//!   * **packing** — a `Workload` with more replicas than nodes converges to N
//!     bound `Placement`s, a node hosting more than one, each with its own
//!     runtime token persisted and verifiable;
//!   * **scale-down cleanup** — lowering `replicas` drops the excess
//!     `Placement`s *and deletes their runtime tokens from OpenBao*, so the node
//!     can no longer fetch a token for a replica that no longer exists (the
//!     running replica is drained by the node; estate-wide token revocation is the
//!     `RevocationIntent`, not this cleanup).
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, Reconciler, SchedulerController, SyncStats,
};
use aog_estate::{
    AttestationProfile, CapabilitySpec, Capacity, Kind, NodeSpec, NodeStatus, Resource,
    ResourceObject, WorkloadKind, WorkloadSpec,
};
use chrono::Utc;
use fabric_contracts::{Budget, Classification, Route, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-o1-deploy";
const PREFIX: &str = "kv/data/loom-deploy";
const CAP: &str = "cap-deploy";
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

/// Stand up an approle with read/write/delete on the deploy KV prefix and return
/// its (role_id, secret_id). Idempotent — safe to call once per test.
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
path "kv/data/loom-deploy/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-deploy/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-o1-deploy",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-o1-deploy","token_ttl":"15m"})),
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

/// A ready node with room for `slots` workloads.
async fn ready_node(client: &EstateClient, name: &str, slots: u32) {
    let capacity = Capacity {
        cpu_millis: 8000,
        memory_mb: 16384,
        gpu: 0,
        max_workloads: slots,
    };
    client
        .ensure_created(ResourceObject::Node(Resource::new(
            name,
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Secret,
                attestation: AttestationProfile::default(),
                capacity,
            },
        )))
        .await
        .unwrap();
    let Some(ResourceObject::Node(mut node)) = client.get(Kind::Node, name).await.unwrap() else {
        panic!("node {name} missing after create");
    };
    node.status = Some(NodeStatus {
        ready: true,
        last_heartbeat: Some(Utc::now().to_rfc3339()),
        allocatable: capacity,
        ..NodeStatus::default()
    });
    client.update(ResourceObject::Node(node)).await.unwrap();
}

fn workload_spec(replicas: u32) -> WorkloadSpec {
    WorkloadSpec {
        workload_kind: WorkloadKind::Gateway,
        replicas,
        ring: 1,
        classification_ceiling: Classification::Internal,
        image: None,
        command: Vec::new(),
        capability: Some(CAP.to_owned()),
    }
}

/// Boot an apiserver + estate client + OpenBao auth over a fresh state dir.
async fn harness(
    addr: &str,
    role_id: String,
    secret_id: String,
    dir: &str,
) -> (
    AppState,
    EstateClient,
    Arc<OpenBaoAuth>,
    Arc<RustCryptoMlDsa87>,
) {
    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-o1-anchor").unwrap());
    let state = AppState::bootstrap(
        1,
        fresh_dir(dir),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let openbao = Arc::new(OpenBaoAuth::new(OpenBaoConfig::new(addr, role_id, secret_id)).unwrap());
    (state, client, openbao, anchor)
}

async fn seed_capability(client: &EstateClient) {
    let cap = CapabilitySpec {
        budget: Budget {
            token_cap: 5000,
            ..Budget::default()
        },
        caveats: vec![],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec!["gpt-x".to_owned()],
        max_classification: Classification::Internal,
        ttl_seconds: 3600,
    };
    let mut r = Resource::new(CAP, cap);
    r.metadata.tenant = Some(TENANT.to_owned());
    client
        .ensure_created(ResourceObject::Capability(r))
        .await
        .unwrap();
}

async fn placements_of(client: &EstateClient, workload: &str) -> Vec<aog_estate::Placement> {
    let mut ps: Vec<_> = client
        .list(Kind::Placement)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|o| match o {
            ResourceObject::Placement(p) if p.spec.workload == workload => Some(p),
            _ => None,
        })
        .collect();
    ps.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    ps
}

#[tokio::test]
async fn packs_replicas_beyond_node_count() {
    let Some(addr) = openbao_addr() else {
        eprintln!("SKIP packs_replicas_beyond_node_count: WSF_OPENBAO_ADDR unset (O1 live gate)");
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;
    let (state, client, openbao, anchor) = harness(&addr, role_id, secret_id, "loom-o1-pack").await;
    let signer: Arc<dyn Signer> = anchor.clone();

    seed_capability(&client).await;
    // Two nodes, room for two workloads each; three replicas must pack.
    ready_node(&client, "node-a", 2).await;
    ready_node(&client, "node-b", 2).await;
    let mut wl = Resource::new("gw", workload_spec(3));
    wl.metadata.tenant = Some(TENANT.to_owned());
    client
        .ensure_created(ResourceObject::Workload(wl))
        .await
        .unwrap();

    let reconciler = SchedulerController::new(client.clone(), Arc::clone(&openbao), PREFIX, signer);
    let mut controller = Controller::new(
        "scheduler",
        state.informer("Workload/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    let placements = placements_of(&client, "gw").await;
    assert_eq!(
        placements.len(),
        3,
        "all three replicas bound (packed onto two nodes)"
    );
    let mut nodes: Vec<&str> = placements.iter().map(|p| p.spec.node.as_str()).collect();
    nodes.sort_unstable();
    nodes.dedup();
    assert_eq!(nodes.len(), 2, "packed across both nodes, one hosting two");

    // Every replica's token is persisted and verifies against the anchor.
    let vault = openbao.login().await.unwrap();
    for p in &placements {
        let data = openbao
            .get_kv_data(&vault, &format!("{PREFIX}/{}", p.metadata.name))
            .await
            .unwrap();
        let token: TrustToken =
            serde_json::from_value(data.get("token").expect("token stored").clone()).unwrap();
        assert!(
            fabric_token::verify(&token, &MlDsa87Verifier, anchor.public_key()).is_ok(),
            "runtime token for {} verifies",
            p.metadata.name
        );
    }
}

#[tokio::test]
async fn scale_down_removes_the_dropped_replicas_token() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP scale_down_removes_the_dropped_replicas_token: WSF_OPENBAO_ADDR unset (O1 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;
    let (state, client, openbao, anchor) =
        harness(&addr, role_id, secret_id, "loom-o1-scaledown").await;
    let signer: Arc<dyn Signer> = anchor.clone();

    seed_capability(&client).await;
    ready_node(&client, "node-a", 4).await;
    ready_node(&client, "node-b", 4).await;
    let mut wl = Resource::new("gw", workload_spec(2));
    wl.metadata.tenant = Some(TENANT.to_owned());
    client
        .ensure_created(ResourceObject::Workload(wl))
        .await
        .unwrap();

    let reconciler = SchedulerController::new(client.clone(), Arc::clone(&openbao), PREFIX, signer);
    let mut controller = Controller::new(
        "scheduler",
        state.informer("Workload/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;
    assert_eq!(
        placements_of(&client, "gw").await.len(),
        2,
        "two replicas bound"
    );

    // The highest ordinal (gw-r1) is the one scale-down will drop; capture its
    // token path and confirm a token lives there now.
    let vault = openbao.login().await.unwrap();
    let dropped_path = format!("{PREFIX}/gw-r1");
    let before = openbao.get_kv_data(&vault, &dropped_path).await.unwrap();
    assert!(
        before.get("token").is_some(),
        "gw-r1 token is live before scale-down"
    );

    // Scale to one replica and let the controller converge.
    let Some(ResourceObject::Workload(mut wl)) = client.get(Kind::Workload, "gw").await.unwrap()
    else {
        panic!("workload gw missing");
    };
    wl.spec.replicas = 1;
    client.update(ResourceObject::Workload(wl)).await.unwrap();
    settle(&mut controller).await;

    // Exactly the ordinal-0 replica survives; gw-r1 is gone from the estate…
    let survivors = placements_of(&client, "gw").await;
    assert_eq!(survivors.len(), 1, "scaled down to one replica");
    assert_eq!(
        survivors[0].metadata.name, "gw-r0",
        "the lowest ordinal survives"
    );
    // …and its runtime token is gone from OpenBao — a read no longer yields one.
    // KV-v2 soft-delete leaves a null-data read; a hard delete 404s. Either way
    // the node can no longer fetch a token for the replica that no longer exists.
    let after = openbao.get_kv_data(&vault, &dropped_path).await;
    let token_gone = match after {
        Err(_) => true,
        Ok(v) => v.get("token").is_none(),
    };
    assert!(
        token_gone,
        "the dropped replica's token is no longer fetchable after scale-down"
    );
}
