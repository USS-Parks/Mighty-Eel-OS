//! O2 gate — a `RolloutPlan` advances through availability-safe steps to `Ready`,
//! and **each step is receipted**: every advance is an admitted status write, so
//! the tamper-evident ledger gains one receipt per step (K9). A rollout whose
//! target does not exist holds `Degraded` (fail-closed) rather than pretending to
//! roll a phantom. The availability floor itself is proven exhaustively by the
//! pure stepper's unit tests (`rollout.rs`); here we prove the controller drives
//! it and leaves the receipt trail.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, Reconciler, RolloutController, SyncStats, total_steps,
};
use aog_estate::{
    Kind, Phase, Resource, ResourceObject, RolloutPlan, RolloutPlanSpec, RolloutStrategy, Workload,
    WorkloadKind, WorkloadSpec,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-o2-anchor").unwrap();
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

/// Drive the controller with a virtual clock (advancing past each `RequeueAfter`)
/// until it settles — deterministic and fast, no real sleeps.
async fn settle<R: Reconciler>(c: &mut Controller<R>) {
    let mut now = Instant::now();
    for _ in 0..100 {
        let s = c.sync(now).await.unwrap();
        if quiet(s) && idle(c) {
            return;
        }
        now += Duration::from_millis(250);
    }
    panic!("controller did not settle");
}

async fn workload(client: &EstateClient, name: &str, replicas: u32) {
    client
        .ensure_created(ResourceObject::Workload(Workload::new(
            name,
            WorkloadSpec {
                workload_kind: WorkloadKind::Gateway,
                replicas,
                ring: 1,
                classification_ceiling: fabric_contracts::Classification::Internal,
                image: None,
                command: Vec::new(),
                capability: None,
            },
        )))
        .await
        .unwrap();
}

async fn get_plan(client: &EstateClient, name: &str) -> RolloutPlan {
    let Some(ResourceObject::RolloutPlan(plan)) =
        client.get(Kind::RolloutPlan, name).await.unwrap()
    else {
        panic!("rollout plan {name} missing");
    };
    plan
}

#[tokio::test]
async fn progressive_rollout_advances_to_ready_receipting_each_step() {
    let state = app_state("loom-o2-progressive").await;
    let client = EstateClient::new(state.admission(), state.reader());

    // A 4-replica workload and a progressive rollout over it: window = surge1 +
    // unavail1 = 2, so it completes in ceil(4/2) = 2 steps.
    workload(&client, "gw", 4).await;
    client
        .ensure_created(ResourceObject::RolloutPlan(Resource::new(
            "roll-gw",
            RolloutPlanSpec {
                target: "gw".to_owned(),
                strategy: RolloutStrategy::Progressive,
                max_surge: 1,
                max_unavailable: 1,
            },
        )))
        .await
        .unwrap();

    let steps = total_steps(RolloutStrategy::Progressive, 4, 1, 1);
    assert_eq!(steps, 2, "window 2 over 4 replicas");

    // Receipts already written by the two creates; measure only the rollout's.
    let before = state.receipts_len();

    let reconciler = RolloutController::new(client.clone());
    let mut controller = Controller::new(
        "rollout",
        state.informer("RolloutPlan/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    let plan = get_plan(&client, "roll-gw").await;
    let status = plan.status.expect("rollout status");
    assert_eq!(status.phase, Phase::Ready, "the rollout completed");
    assert_eq!(status.step, 2, "it reached the terminal step");

    let receipted = state.receipts_len() - before;
    assert!(
        receipted >= steps,
        "each rollout step is receipted (>= {steps} receipts, got {receipted})"
    );
}

#[tokio::test]
async fn a_rollout_with_a_missing_target_is_degraded() {
    let state = app_state("loom-o2-degraded").await;
    let client = EstateClient::new(state.admission(), state.reader());

    // No such workload "ghost": the rollout must not pretend to progress.
    client
        .ensure_created(ResourceObject::RolloutPlan(Resource::new(
            "roll-ghost",
            RolloutPlanSpec {
                target: "ghost".to_owned(),
                strategy: RolloutStrategy::Progressive,
                max_surge: 1,
                max_unavailable: 1,
            },
        )))
        .await
        .unwrap();

    let reconciler = RolloutController::new(client.clone());
    let mut controller = Controller::new(
        "rollout",
        state.informer("RolloutPlan/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    let status = get_plan(&client, "roll-ghost")
        .await
        .status
        .expect("status");
    assert_eq!(status.phase, Phase::Degraded, "no target → no rollout");
    assert_eq!(status.step, 0, "it never advanced");
}
