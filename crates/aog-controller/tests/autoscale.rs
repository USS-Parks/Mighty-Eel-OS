//! O4 gate (controller side) — the autoscaler applies its decision by editing
//! `Workload.spec.replicas`: one reconcile pass under saturation adds a replica,
//! one pass while idle removes one. The decision's determinism and
//! budget-respect (saturated-but-broke → recommend hardware, never overspend;
//! never below min) are proven exhaustively by the pure `autoscale` unit tests;
//! here we prove the controller drives the estate from that decision.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, AutoscaleController, AutoscalePolicy, AutoscaleProbe, AutoscaleSignals,
    Controller, EstateClient,
};
use aog_estate::{Kind, ResourceObject, Workload, WorkloadKind, WorkloadSpec};
use fabric_contracts::Classification;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;

/// Reports a fixed signal snapshot for every workload.
struct FixedProbe(AutoscaleSignals);

impl AutoscaleProbe for FixedProbe {
    fn signals(&self, _workload: &str) -> Option<AutoscaleSignals> {
        Some(self.0)
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-o4-anchor").unwrap();
    AppState::bootstrap(
        1,
        fresh_dir(dir),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap()
}

async fn workload(client: &EstateClient, name: &str, replicas: u32) {
    client
        .ensure_created(ResourceObject::Workload(Workload::new(
            name,
            WorkloadSpec {
                workload_kind: WorkloadKind::Gateway,
                replicas,
                ring: 1,
                classification_ceiling: Classification::Internal,
                image: None,
                command: Vec::new(),
                capability: None,
            },
        )))
        .await
        .unwrap();
}

async fn replicas_of(client: &EstateClient, name: &str) -> u32 {
    let Some(ResourceObject::Workload(wl)) = client.get(Kind::Workload, name).await.unwrap() else {
        panic!("workload {name} missing");
    };
    wl.spec.replicas
}

fn signals(utilization: f64, budget_headroom: f64, roi: f64) -> AutoscaleSignals {
    AutoscaleSignals {
        utilization,
        budget_headroom,
        roi,
    }
}

#[tokio::test]
async fn a_saturated_workload_scales_up_one_replica_per_pass() {
    let state = app_state("loom-o4-up").await;
    let client = EstateClient::new(state.admission(), state.reader());
    workload(&client, "gw", 2).await;

    // Saturated, budget healthy → scale up.
    let probe = Arc::new(FixedProbe(signals(0.95, 0.9, 0.9)));
    let reconciler = AutoscaleController::new(client.clone(), AutoscalePolicy::default(), probe);
    let mut controller = Controller::new(
        "autoscale",
        state.informer("Workload/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    // One reconcile pass = one scale step.
    controller.sync(Instant::now()).await.unwrap();

    assert_eq!(
        replicas_of(&client, "gw").await,
        3,
        "a saturated workload gains a replica"
    );
}

#[tokio::test]
async fn an_idle_workload_consolidates_down_one_replica_per_pass() {
    let state = app_state("loom-o4-down").await;
    let client = EstateClient::new(state.admission(), state.reader());
    workload(&client, "gw", 3).await;

    // Idle, budget healthy → consolidate down.
    let probe = Arc::new(FixedProbe(signals(0.05, 0.9, 0.9)));
    let reconciler = AutoscaleController::new(client.clone(), AutoscalePolicy::default(), probe);
    let mut controller = Controller::new(
        "autoscale",
        state.informer("Workload/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    controller.sync(Instant::now()).await.unwrap();

    assert_eq!(
        replicas_of(&client, "gw").await,
        2,
        "an idle workload sheds a replica"
    );
}
