//! X1 + R5 gate — "budgets hold across ≥3 replicas under load (live);
//! over-spend ≤ ε; no per-call shared-store round-trip", and "concurrent
//! decrement across 3 apiserver clients never over-spends a cap (live)".
//!
//! Env-gated on `WSF_OPENBAO_ADDR` like the other live suites. Three
//! `LeasedSpendLedger` replicas race one capability's budget through the
//! OpenBao-KV-CAS lease store — genuinely concurrent, genuinely shared.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::capability::spend_key;
use aog_controller::{
    AlwaysLeader, CapabilityController, Controller, EstateClient, Reconciler, SyncStats,
};
use aog_estate::{CapabilitySpec, Kind, Phase, Resource, ResourceObject};
use aog_gateway::spend::OpenBaoLeaseStore;
use fabric_contracts::{Budget, Classification};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use fabric_token::spend::{LeaseStore, LeasedSpendLedger, SpendError, Spent};
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-r5-spend";
const SPEND_PREFIX: &str = "kv/data/spend";

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
path "kv/data/spend/*" { capabilities = ["create", "read", "update"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r5-spend",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r5-spend","token_ttl":"15m"})),
    )
    .await;

    // A fresh shared pool: drop any record a prior run leased against (KV v2
    // metadata delete removes all versions, resetting the CAS counter).
    let _ = bao(
        c,
        addr,
        tok,
        Method::DELETE,
        "kv/metadata/spend/cap-shared-cap",
        None,
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

async fn settle1<R: Reconciler>(controller: &mut Controller<R>) {
    for _ in 0..100 {
        let stats = controller.sync(Instant::now()).await.unwrap();
        if quiet(stats) && controller.queue_len() == 0 && controller.delayed_len() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("controller did not settle within 100 rounds");
}

/// Counts shared-store acquisitions — the round-trip amortization evidence.
struct CountingStore {
    inner: OpenBaoLeaseStore,
    acquisitions: Arc<AtomicU32>,
}

impl LeaseStore for CountingStore {
    fn acquire(
        &self,
        key: &str,
        cap: &Budget,
        want: Spent,
    ) -> impl std::future::Future<Output = Result<Spent, SpendError>> + Send {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        self.inner.acquire(key, cap, want)
    }
}

fn usd(cents: u64) -> Spent {
    Spent {
        usd_cents: cents,
        ..Spent::default()
    }
}

#[tokio::test]
async fn three_replicas_never_over_spend_a_capability_cap_live() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP three_replicas_never_over_spend_a_capability_cap_live: WSF_OPENBAO_ADDR unset (X1/R5 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    // The estate: one Capability with a $100.00 budget, reconciled Ready.
    let anchor = RustCryptoMlDsa87::generate("loom-r5-anchor").unwrap();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r5-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let cap_budget = Budget {
        usd_cap_cents: 10_000,
        ..Default::default()
    };
    client
        .ensure_created(ResourceObject::Capability(Resource::new(
            "shared-cap",
            CapabilitySpec {
                budget: cap_budget.clone(),
                caveats: vec![],
                allowed_routes: vec![],
                allowed_models: vec![],
                max_classification: Classification::Restricted,
                ttl_seconds: 300,
            },
        )))
        .await
        .unwrap();
    let mut capabilities = Controller::new(
        "capability",
        state.informer("Capability/"),
        CapabilityController::new(client.clone()),
        Arc::new(AlwaysLeader),
    );
    settle1(&mut capabilities).await;
    let Some(ResourceObject::Capability(capability)) =
        client.get(Kind::Capability, "shared-cap").await.unwrap()
    else {
        panic!("capability missing");
    };
    assert_eq!(
        capability.status.expect("status written").phase,
        Phase::Ready
    );

    // Three replicas, one shared atomic record, genuinely concurrent load.
    // Slice = $5.00 → published ε = 3 × $5.00 = $15.00 of the $100.00 cap.
    let key = spend_key("shared-cap");
    let acquisitions = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();
    for _ in 0..3 {
        let store = CountingStore {
            inner: OpenBaoLeaseStore::new(
                OpenBaoAuth::new(OpenBaoConfig::new(
                    &addr,
                    role_id.clone(),
                    secret_id.clone(),
                ))
                .unwrap(),
                SPEND_PREFIX,
            )
            .unwrap(),
            acquisitions: Arc::clone(&acquisitions),
        };
        let ledger = LeasedSpendLedger::new(store, usd(500));
        let cap = cap_budget.clone();
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            let mut approved = 0u64;
            let mut calls = 0u32;
            // 60 × $1.00 per replica = $180.00 attempted against a $100.00 cap.
            for _ in 0..60 {
                calls += 1;
                if ledger.try_spend(&key, &cap, usd(100)).await.unwrap() {
                    approved += 100;
                }
            }
            (approved, calls)
        }));
    }
    let mut total_approved = 0u64;
    let mut total_calls = 0u32;
    for handle in handles {
        let (approved, calls) = handle.await.unwrap();
        total_approved += approved;
        total_calls += calls;
    }

    // The R5 gate: concurrent decrement never over-spends the cap…
    assert!(
        total_approved <= 10_000,
        "over-spend: {total_approved} > 10000"
    );
    // …the X1 gate: under-utilization bounded by the published ε…
    assert!(
        total_approved >= 10_000 - 3 * 500,
        "under-utilized beyond ε: {total_approved}"
    );
    // …and the shared record itself never leased past the cap.
    let record: serde_json::Value = bao(
        &http,
        &addr,
        &root_token(),
        Method::GET,
        &format!("{SPEND_PREFIX}/{key}"),
        None,
    )
    .await
    .json()
    .await
    .expect("shared spend record");
    let leased = record["data"]["data"]["leased"]["usd_cents"]
        .as_u64()
        .expect("leased usd");
    assert!(leased <= 10_000, "shared record leased {leased} > cap");

    // No per-call round-trip: acquisitions ≪ calls (amortized by the slice).
    let trips = acquisitions.load(Ordering::SeqCst);
    assert!(
        trips < total_calls && trips <= 40,
        "store round-trips not amortized: {trips} trips for {total_calls} calls"
    );
}
