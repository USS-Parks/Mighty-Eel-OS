//! O7 gate (controller side) — a cordoned node drains **within the disruption
//! budget**: one pass evicts at most `budget` replicas of a workload, and the
//! node fully drains over successive passes; an uncordoned node is untouched. The
//! budget arithmetic and determinism are proven by the pure `plan_drain` unit
//! tests; ring preservation is structural (the scheduler re-places drained
//! replicas through the unchanged S3 ring filter) and asserted in the live gate.

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

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-o7-anchor").unwrap();
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

/// Drive past each 5s drain requeue until the node settles.
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

async fn node(client: &EstateClient, name: &str, cordoned: bool) {
    let mut n = Node::new(
        name,
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: Capacity::default(),
        },
    );
    if cordoned {
        n.metadata
            .labels
            .insert(CORDON_LABEL.to_owned(), "true".to_owned());
    }
    client
        .ensure_created(ResourceObject::Node(n))
        .await
        .unwrap();
}

async fn placement(client: &EstateClient, name: &str, workload: &str, on_node: &str) {
    client
        .ensure_created(ResourceObject::Placement(Resource::new(
            name,
            PlacementSpec {
                workload: workload.to_owned(),
                node: on_node.to_owned(),
                token_id: format!("rt:{name}"),
            },
        )))
        .await
        .unwrap();
}

async fn placements_on(client: &EstateClient, node: &str) -> usize {
    client
        .list(Kind::Placement)
        .await
        .unwrap()
        .into_iter()
        .filter(|o| matches!(o, ResourceObject::Placement(p) if p.spec.node == node))
        .count()
}

#[tokio::test]
async fn a_cordoned_node_drains_within_the_disruption_budget() {
    let state = app_state("loom-o7-drain").await;
    let client = EstateClient::new(state.admission(), state.reader());

    node(&client, "node-a", true).await; // cordoned
    for i in 0..4 {
        placement(&client, &format!("gw-r{i}"), "gw", "node-a").await;
    }

    // Disruption budget 2: no more than two gw replicas may be down at once.
    let reconciler = MaintenanceController::new(client.clone(), 2);
    let mut controller = Controller::new(
        "maintenance",
        state.informer("Node/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );

    // One pass evicts exactly the budget (2), leaving 2 in place.
    controller.sync(Instant::now()).await.unwrap();
    assert_eq!(
        placements_on(&client, "node-a").await,
        2,
        "at most the disruption budget comes down in one pass"
    );

    // Over further passes the node fully drains.
    settle(&mut controller).await;
    assert_eq!(
        placements_on(&client, "node-a").await,
        0,
        "the cordoned node fully drains"
    );
}

#[tokio::test]
async fn an_uncordoned_node_is_not_drained() {
    let state = app_state("loom-o7-nodrain").await;
    let client = EstateClient::new(state.admission(), state.reader());

    node(&client, "node-b", false).await; // not cordoned
    placement(&client, "gw-r0", "gw", "node-b").await;
    placement(&client, "gw-r1", "gw", "node-b").await;

    let reconciler = MaintenanceController::new(client.clone(), 1);
    let mut controller = Controller::new(
        "maintenance",
        state.informer("Node/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    assert_eq!(
        placements_on(&client, "node-b").await,
        2,
        "an uncordoned node keeps all its placements"
    );
}
