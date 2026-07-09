//! "an unhealthy provider is removed from the schedulable set within
//! SLO." Real HTTP end to end (no mock): a live local server stands in for the
//! provider; the `HttpHealthProbe` issues real liveness GETs, and the
//! controller folds the result into `ProviderPool.status.healthy`. Flipping the
//! server unhealthy drops its models from the set on the next resync heartbeat;
//! recovery re-adds them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, HttpHealthProbe, ProviderPoolController, Reconciler,
    SyncStats,
};
use aog_estate::{
    Kind, ModelEndpoint, Phase, ProviderPool, ProviderPoolSpec, Resource, ResourceObject,
};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use fabric_contracts::Route;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use tokio::net::TcpListener;

/// A local health endpoint whose status code the test controls.
async fn healthz(State(code): State<Arc<AtomicU16>>) -> StatusCode {
    StatusCode::from_u16(code.load(Ordering::SeqCst)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// Spawn a provider health server; returns its base URL and the status handle.
async fn spawn_provider() -> (String, Arc<AtomicU16>) {
    let code = Arc::new(AtomicU16::new(200));
    let app = Router::new()
        .route("/healthz", get(healthz))
        .with_state(Arc::clone(&code));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, code)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn endpoint(model: &str, declared: bool) -> ModelEndpoint {
    ModelEndpoint {
        model: model.to_owned(),
        route: Route::CloudAllowed,
        cost_cents_per_ktoken: 0,
        healthy: declared,
    }
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-r7-anchor").unwrap();
    AppState::bootstrap(
        1,
        fresh_dir(dir),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap()
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

async fn get_pool(client: &EstateClient, name: &str) -> ProviderPool {
    let Some(ResourceObject::ProviderPool(pool)) =
        client.get(Kind::ProviderPool, name).await.unwrap()
    else {
        panic!("pool missing");
    };
    pool
}

#[tokio::test]
async fn an_unhealthy_provider_drops_from_the_schedulable_set() {
    let (base, code) = spawn_provider().await;
    let state = app_state("loom-r7-http").await;
    let client = EstateClient::new(state.admission(), state.reader());

    // A provider pool with two cloud endpoints — declared `healthy: false` so
    // the live probe (not the flag) is proven to govern.
    client
        .ensure_created(ResourceObject::ProviderPool(Resource::new(
            "acme-pool",
            ProviderPoolSpec {
                provider: "acme".to_owned(),
                endpoints: vec![endpoint("gpt-x", false), endpoint("gpt-mini", false)],
            },
        )))
        .await
        .unwrap();

    let probe = Arc::new(HttpHealthProbe::new(HashMap::from([("acme".to_owned(), base)])).unwrap());
    let reconciler = ProviderPoolController::new(client.clone(), probe);
    let mut controller = Controller::new(
        "providerpool",
        state.informer("ProviderPool/"),
        reconciler,
        Arc::new(AlwaysLeader),
    )
    .with_resync(Duration::from_secs(10));
    settle(&mut controller).await;

    // ── Provider up: both models are schedulable, despite `healthy: false`.
    let status = get_pool(&client, "acme-pool").await.status.expect("status");
    assert_eq!(status.phase, Phase::Ready);
    assert_eq!(
        status.healthy,
        vec!["gpt-mini".to_owned(), "gpt-x".to_owned()],
        "a live provider's endpoints are schedulable"
    );

    // ── Provider goes down (503). The resync heartbeat re-probes.
    code.store(503, Ordering::SeqCst);
    controller
        .sync(Instant::now() + Duration::from_secs(20))
        .await
        .unwrap();
    settle(&mut controller).await;
    let status = get_pool(&client, "acme-pool").await.status.expect("status");
    assert_eq!(
        status.phase,
        Phase::Degraded,
        "an unhealthy provider has nothing schedulable"
    );
    assert!(
        status.healthy.is_empty(),
        "the unhealthy provider is removed from the schedulable set"
    );

    // ── Recovery: the endpoints rejoin the schedulable set.
    code.store(200, Ordering::SeqCst);
    controller
        .sync(Instant::now() + Duration::from_secs(40))
        .await
        .unwrap();
    settle(&mut controller).await;
    let status = get_pool(&client, "acme-pool").await.status.expect("status");
    assert_eq!(status.phase, Phase::Ready);
    assert_eq!(
        status.healthy,
        vec!["gpt-mini".to_owned(), "gpt-x".to_owned()],
        "a recovered provider is schedulable again"
    );
}

#[tokio::test]
async fn a_provider_without_a_probe_url_uses_declared_health() {
    let state = app_state("loom-r7-declared").await;
    let client = EstateClient::new(state.admission(), state.reader());

    // A local (air-gapped) provider: no probe URL, so the declared flag governs.
    client
        .ensure_created(ResourceObject::ProviderPool(Resource::new(
            "local-pool",
            ProviderPoolSpec {
                provider: "local-mai".to_owned(),
                endpoints: vec![endpoint("llama-70b", true), endpoint("llama-7b", false)],
            },
        )))
        .await
        .unwrap();

    // No base URLs configured — every provider falls back to declared health.
    let probe = Arc::new(HttpHealthProbe::new(HashMap::new()).unwrap());
    let reconciler = ProviderPoolController::new(client.clone(), probe);
    let mut controller = Controller::new(
        "providerpool",
        state.informer("ProviderPool/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    let status = get_pool(&client, "local-pool")
        .await
        .status
        .expect("status");
    assert_eq!(status.phase, Phase::Ready);
    assert_eq!(
        status.healthy,
        vec!["llama-70b".to_owned()],
        "only the declared-healthy endpoint is schedulable"
    );
}
