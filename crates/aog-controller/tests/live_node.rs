//! N2 gate — "a killed node reschedules its workloads", against a **live**
//! OpenBao (A3.2). Two of three ready nodes host a 2-replica workload's replicas;
//! one node's heartbeat goes stale; the node controller marks it down and evicts
//! its placement; a fresh scheduler pass re-places the freed replica on the idle
//! live node. The replica moves off the dead node onto a live one.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, NodeController, Reconciler, SchedulerController,
    SyncStats,
};
use aog_estate::{
    AttestationProfile, CapabilitySpec, Capacity, Kind, NodeSpec, NodeStatus, Resource,
    ResourceObject, WorkloadKind, WorkloadSpec,
};
use chrono::Utc;
use fabric_contracts::{Budget, Classification, Route};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-n2-node";
const PREFIX: &str = "kv/data/loom-n2";
const WORKLOAD: &str = "gw";
const CAP: &str = "cap-n2";
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
path "kv/data/loom-n2/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-n2/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-n2-node",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-n2-node","token_ttl":"15m"})),
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

async fn ready_node(client: &EstateClient, name: &str) {
    let capacity = Capacity {
        cpu_millis: 8000,
        memory_mb: 16384,
        gpu: 0,
        max_workloads: 4,
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

/// Kill a node: its last heartbeat is now two minutes old (it stopped beating).
async fn kill_node(client: &EstateClient, name: &str) {
    let Some(ResourceObject::Node(mut node)) = client.get(Kind::Node, name).await.unwrap() else {
        panic!("node {name} missing");
    };
    let mut status = node.status.take().unwrap_or_default();
    status.last_heartbeat = Some((Utc::now() - chrono::Duration::seconds(120)).to_rfc3339());
    node.status = Some(status);
    client.update(ResourceObject::Node(node)).await.unwrap();
}

async fn placement_nodes(client: &EstateClient) -> Vec<String> {
    let mut nodes: Vec<String> = client
        .list(Kind::Placement)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|o| match o {
            ResourceObject::Placement(p) if p.spec.workload == WORKLOAD => Some(p.spec.node),
            _ => None,
        })
        .collect();
    nodes.sort();
    nodes
}

fn capability() -> CapabilitySpec {
    CapabilitySpec {
        budget: Budget {
            token_cap: 5000,
            ..Budget::default()
        },
        caveats: vec![],
        allowed_routes: vec![Route::LocalOnly],
        allowed_models: vec!["gpt-x".to_owned()],
        max_classification: Classification::Internal,
        ttl_seconds: 3600,
    }
}

fn workload() -> WorkloadSpec {
    WorkloadSpec {
        workload_kind: WorkloadKind::Gateway,
        replicas: 2,
        ring: 1,
        classification_ceiling: Classification::Internal,
        image: None,
        command: Vec::new(),
        capability: Some(CAP.to_owned()),
    }
}

#[tokio::test]
async fn a_killed_node_reschedules_its_workload() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP a_killed_node_reschedules_its_workload: WSF_OPENBAO_ADDR unset (N2 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-n2-anchor").unwrap());
    let signer: Arc<dyn Signer> = anchor.clone();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-n2-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let openbao =
        Arc::new(OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap());

    client
        .ensure_created(ResourceObject::Capability(Resource::new(CAP, capability())))
        .await
        .unwrap();
    for n in ["node-a", "node-b", "node-c"] {
        ready_node(&client, n).await;
    }
    let mut wl = Resource::new(WORKLOAD, workload());
    wl.metadata.tenant = Some(TENANT.to_owned());
    client
        .ensure_created(ResourceObject::Workload(wl))
        .await
        .unwrap();

    // Initial placement: the two replicas land on node-a and node-b.
    let sched =
        SchedulerController::new(client.clone(), Arc::clone(&openbao), PREFIX, signer.clone());
    let mut sc = Controller::new(
        "scheduler",
        state.informer("Workload/"),
        sched,
        Arc::new(AlwaysLeader),
    );
    settle(&mut sc).await;
    assert_eq!(
        placement_nodes(&client).await,
        vec!["node-a".to_owned(), "node-b".to_owned()]
    );

    // Kill node-a; the node controller marks it down and evicts its placement.
    kill_node(&client, "node-a").await;
    let node_ctl = NodeController::new(client.clone(), 30);
    let mut nc = Controller::new(
        "node",
        state.informer("Node/"),
        node_ctl,
        Arc::new(AlwaysLeader),
    );
    settle(&mut nc).await;

    // A fresh scheduler pass re-places the freed replica on the idle live node.
    let sched2 = SchedulerController::new(client.clone(), Arc::clone(&openbao), PREFIX, signer);
    let mut sc2 = Controller::new(
        "scheduler",
        state.informer("Workload/"),
        sched2,
        Arc::new(AlwaysLeader),
    );
    settle(&mut sc2).await;

    assert_eq!(
        placement_nodes(&client).await,
        vec!["node-b".to_owned(), "node-c".to_owned()],
        "the replica rescheduled off the killed node onto a live one"
    );
}
