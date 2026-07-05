//! R6 gate — "a bundle update reaches all nodes; signature verifies at the
//! edge; stale bundle rejected", against a **live** OpenBao (A3.2 no-mock-only).
//!
//! Env-gated on `WSF_OPENBAO_ADDR` like the R3/R4 live suites; returns cleanly
//! otherwise. Real crypto end to end: the controller signs each `PolicyBundle`
//! with an ML-DSA control-plane key and publishes it to the live KV-v2 channel
//! `kv/data/policy-bundles/<name>`; an edge cache fetches it and verifies with
//! the public key **alone** (no control-plane contact), accepts a forward
//! version, and refuses both a replayed older bundle (anti-rollback) and a
//! tampered one. The controller itself refuses to regress the channel to a
//! stale spec.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, BundleReject, BundleStore, Controller, EdgeBundleCache, EstateClient,
    OpenBaoBundleStore, PolicyBundleController, Reconciler, SignedBundle, SyncStats, verify_bundle,
};
use aog_estate::{
    Kind, NodeSpec, Phase, PolicyBundleSpec, PolicyMode, PolicyRule, Resource, ResourceObject,
    WorkloadKind, WorkloadSpec,
};
use fabric_contracts::{Classification, RoutingDecision};
use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};

const ROLE: &str = "loom-r6-bundle";
const BUNDLE: &str = "estate-baseline";

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

/// Bootstrap: mount KV-v2 (root), and an AppRole with policy-bundle CRUD.
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
    // Already-mounted `kv` returns 400 — harmless.
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
path "kv/data/policy-bundles/*"     { capabilities = ["create", "read", "update", "delete"] }
path "kv/metadata/policy-bundles/*" { capabilities = ["read", "delete", "list"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r6-bundle",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r6-bundle","token_ttl":"15m"})),
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

fn rule(name: &str, effect: RoutingDecision) -> PolicyRule {
    PolicyRule {
        name: name.to_owned(),
        effect,
        when: String::new(),
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

async fn get_bundle(client: &EstateClient) -> aog_estate::PolicyBundle {
    let Some(ResourceObject::PolicyBundle(b)) =
        client.get(Kind::PolicyBundle, BUNDLE).await.unwrap()
    else {
        panic!("bundle missing");
    };
    b
}

#[tokio::test]
async fn a_policy_bundle_is_signed_distributed_and_verified_at_the_edge() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP a_policy_bundle_is_signed_distributed_and_verified_at_the_edge: WSF_OPENBAO_ADDR unset (R6 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;
    // Pristine channel: the live KV persists across runs, so purge every prior
    // version of this bundle before the fresh v1 (root destroys all versions).
    bao(
        &http,
        &addr,
        &root_token(),
        Method::DELETE,
        &format!("kv/metadata/policy-bundles/{BUNDLE}"),
        None,
    )
    .await;

    // Control plane: the estate store (redb) + the bundle-signing key.
    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-r6-anchor").unwrap());
    let signer: Arc<dyn Signer> =
        Arc::new(RustCryptoMlDsa87::generate("loom-r6-bundle-signer").unwrap());
    let public_key = signer.public_key().to_vec();
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r6-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let store = Arc::new(
        OpenBaoBundleStore::new(
            OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        )
        .unwrap(),
    );

    // The edges the channel serves: a node and a gateway workload.
    client
        .ensure_created(ResourceObject::Node(Resource::new(
            "edge-node-1",
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Restricted,
                attestation: aog_estate::AttestationProfile::default(),
                capacity: aog_estate::Capacity::default(),
            },
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::Workload(Resource::new(
            "gw-openai",
            WorkloadSpec {
                workload_kind: WorkloadKind::Gateway,
                replicas: 1,
                ring: 1,
                classification_ceiling: Classification::Restricted,
                image: None,
                command: vec![],
                capability: None,
            },
        )))
        .await
        .unwrap();

    // A policy bundle, version 1, in enforce mode.
    client
        .ensure_created(ResourceObject::PolicyBundle(Resource::new(
            BUNDLE,
            PolicyBundleSpec {
                version: 1,
                mode: PolicyMode::Enforce,
                rules: vec![rule("deny-cloud-egress", RoutingDecision::Deny)],
            },
        )))
        .await
        .unwrap();

    let reconciler =
        PolicyBundleController::new(client.clone(), Arc::clone(&store), Arc::clone(&signer));
    let mut controller = Controller::new(
        "policybundle",
        state.informer("PolicyBundle/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // ── Distributed: status Ready, and it reached the node + the gateway.
    let bundle = get_bundle(&client).await;
    let status = bundle.status.clone().expect("bundle status written");
    assert_eq!(status.phase, Phase::Ready);
    assert_eq!(
        status.distributed_to,
        vec!["edge-node-1".to_owned(), "gw-openai".to_owned()],
        "the bundle reached every node and gateway"
    );

    // ── Verified at the edge with the public key alone (no control-plane call).
    let published = store
        .fetch(BUNDLE)
        .await
        .unwrap()
        .expect("bundle on channel");
    verify_bundle(&published, &MlDsa87Verifier, &public_key)
        .expect("published bundle verifies off-host with the public key alone");
    assert_eq!(published.version, 1);
    let v1 = published.clone();

    let mut edge = EdgeBundleCache::new(public_key.clone());
    edge.accept(published).unwrap();
    assert_eq!(edge.applied(BUNDLE).unwrap().version, 1);

    // ── An update reaches the edge: bump to version 2.
    let mut updated = get_bundle(&client).await;
    updated.spec.version = 2;
    updated.spec.rules = vec![
        rule("deny-cloud-egress", RoutingDecision::Deny),
        rule("allow-local", RoutingDecision::Allow),
    ];
    client
        .update(ResourceObject::PolicyBundle(updated))
        .await
        .unwrap();
    settle(&mut controller).await;

    let v2 = store.fetch(BUNDLE).await.unwrap().expect("v2 on channel");
    assert_eq!(v2.version, 2);
    edge.accept(v2).unwrap();
    assert_eq!(edge.applied(BUNDLE).unwrap().version, 2);

    // ── Stale rejected: a replayed, validly-signed v1 cannot downgrade the edge.
    assert_eq!(
        edge.accept(v1),
        Err(BundleReject::Stale {
            applied: 2,
            offered: 1
        }),
        "a replayed older bundle is refused"
    );
    assert_eq!(edge.applied(BUNDLE).unwrap().version, 2);

    // ── The controller refuses to regress the channel to a stale spec.
    let mut regressed = get_bundle(&client).await;
    regressed.spec.version = 1; // behind the live v2
    client
        .update(ResourceObject::PolicyBundle(regressed))
        .await
        .unwrap();
    settle(&mut controller).await;
    assert_eq!(
        get_bundle(&client).await.status.expect("status").phase,
        Phase::Degraded,
        "a stale spec is Degraded, not shipped"
    );
    assert_eq!(
        store.fetch(BUNDLE).await.unwrap().expect("channel").version,
        2,
        "the live channel was not downgraded"
    );

    // ── A tampered artifact on the channel is refused at the edge.
    let mut tampered = SignedBundle {
        version: 3,
        ..store.fetch(BUNDLE).await.unwrap().unwrap()
    };
    tampered
        .rules
        .push(rule("smuggled-allow", RoutingDecision::Allow));
    store.publish(&tampered).await.unwrap(); // published without re-signing
    let fetched_bad = store.fetch(BUNDLE).await.unwrap().unwrap();
    assert_eq!(
        edge.accept(fetched_bad),
        Err(BundleReject::BadSignature),
        "a tampered bundle is refused even though its version is newer"
    );
    assert_eq!(edge.applied(BUNDLE).unwrap().version, 2);
}
