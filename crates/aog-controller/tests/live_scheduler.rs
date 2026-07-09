//! S7 gate — attested binding against a **live** OpenBao (A3.2 no-mock-only).
//! The scheduler controller places a 2-replica `Workload` across two ready
//! nodes, mints a `Capability`-scoped runtime token per replica, persists each
//! token to OpenBao, and creates each `Placement` through admission — which
//! receipts the binding. We assert the placements land on distinct nodes,
//! each carries a token id, and the persisted token verifies against the anchor
//! and carries the capability's scope.
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

const ROLE: &str = "loom-s7-sched";
const PREFIX: &str = "kv/data/loom-sched";
const WORKLOAD: &str = "gw";
const CAP: &str = "cap-sched";
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
path "kv/data/loom-sched/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/loom-sched/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-s7-sched",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-s7-sched","token_ttl":"15m"})),
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

/// Create a node and report it ready with allocatable capacity — the readiness
/// the node agent will publish in Phase N, supplied here directly.
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
async fn scheduler_binds_replicas_with_scoped_tokens() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP scheduler_binds_replicas_with_scoped_tokens: WSF_OPENBAO_ADDR unset (S7 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-s7-anchor").unwrap());
    let signer: Arc<dyn Signer> = anchor.clone();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-s7-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let openbao =
        Arc::new(OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap());

    // A scoped capability, two ready nodes, and a 2-replica workload naming it.
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
    let cap_res = {
        let mut r = Resource::new(CAP, cap.clone());
        r.metadata.tenant = Some(TENANT.to_owned());
        r
    };
    client
        .ensure_created(ResourceObject::Capability(cap_res))
        .await
        .unwrap();
    ready_node(&client, "node-a").await;
    ready_node(&client, "node-b").await;
    let mut wl = Resource::new(WORKLOAD, workload());
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

    // ── Both replicas are bound, on distinct nodes, each with a runtime token.
    let mut placements: Vec<_> = client
        .list(Kind::Placement)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|o| match o {
            ResourceObject::Placement(p) if p.spec.workload == WORKLOAD => Some(p),
            _ => None,
        })
        .collect();
    placements.sort_by(|a, b| a.spec.node.cmp(&b.spec.node));
    assert_eq!(placements.len(), 2, "both replicas bound");
    assert_eq!(placements[0].spec.node, "node-a");
    assert_eq!(placements[1].spec.node, "node-b");
    assert!(
        placements.iter().all(|p| !p.spec.token_id.is_empty()),
        "every binding carries a runtime token id"
    );

    // ── The persisted token verifies against the anchor and carries cap scope.
    let vault = openbao.login().await.unwrap();
    let path = format!("{PREFIX}/{}", placements[0].metadata.name);
    let data = openbao.get_kv_data(&vault, &path).await.unwrap();
    let token: TrustToken =
        serde_json::from_value(data.get("token").expect("token stored").clone()).unwrap();
    assert!(
        fabric_token::verify(&token, &MlDsa87Verifier, anchor.public_key()).is_ok(),
        "the runtime token verifies against the trust anchor"
    );
    assert_eq!(token.token_id, placements[0].spec.token_id);
    assert_eq!(
        token.budget.as_ref().expect("budget").token_cap,
        5000,
        "the token carries the capability's budget"
    );
    assert_eq!(token.allowed_models, vec!["gpt-x".to_owned()]);
    assert_eq!(token.max_data_classification, Classification::Internal);
    assert_eq!(token.tenant_id, TENANT);
}
