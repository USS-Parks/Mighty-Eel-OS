//! O5 gate (controller side) — a `MissionContract` materializes into exactly the
//! `ToolGrant`s its `allowed_tools` name (owned by the contract), and no others:
//! a tool outside the contract has no grant, so the toolproxy (O6) can never mint
//! a credential the mission did not sanction. Shrinking the contract prunes the
//! withdrawn tool's grant. Per-action scope/budget enforcement (a run cannot
//! exceed the contract) is proven exhaustively by the pure `mission_allows` unit
//! tests in `mission.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, MissionContractController, Reconciler, SyncStats,
};
use aog_estate::{
    Kind, MissionContract, MissionContractSpec, Phase, Resource, ResourceObject, ToolGrant,
};
use fabric_contracts::Budget;
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-o5-anchor").unwrap();
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
    let mut now = Instant::now();
    for _ in 0..100 {
        let s = c.sync(now).await.unwrap();
        if quiet(s) && idle(c) {
            return;
        }
        now += Duration::from_millis(50);
    }
    panic!("controller did not settle");
}

async fn mission(
    client: &EstateClient,
    name: &str,
    tools: &[&str],
    systems: &[&str],
    ceiling: u32,
) {
    client
        .ensure_created(ResourceObject::MissionContract(Resource::new(
            name,
            MissionContractSpec {
                allowed_tools: tools.iter().map(|s| (*s).to_owned()).collect(),
                allowed_systems: systems.iter().map(|s| (*s).to_owned()).collect(),
                call_ceiling: ceiling,
                spend: Budget::default(),
            },
        )))
        .await
        .unwrap();
}

async fn get_mission(client: &EstateClient, name: &str) -> MissionContract {
    let Some(ResourceObject::MissionContract(mc)) =
        client.get(Kind::MissionContract, name).await.unwrap()
    else {
        panic!("mission {name} missing");
    };
    mc
}

/// The `ToolGrant`s owned by mission `mission`, sorted by the tool they grant.
async fn grants_of(client: &EstateClient, mission: &str) -> Vec<ToolGrant> {
    let mut grants: Vec<ToolGrant> = client
        .list(Kind::ToolGrant)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|o| match o {
            ResourceObject::ToolGrant(g)
                if g.metadata
                    .owner_refs
                    .iter()
                    .any(|r| r.kind == Kind::MissionContract && r.name == mission) =>
            {
                Some(g)
            }
            _ => None,
        })
        .collect();
    grants.sort_by(|a, b| a.spec.tool.cmp(&b.spec.tool));
    grants
}

fn controller(state: &AppState, client: &EstateClient) -> Controller<MissionContractController> {
    Controller::new(
        "mission",
        state.informer("MissionContract/"),
        MissionContractController::new(client.clone()),
        Arc::new(AlwaysLeader),
    )
}

#[tokio::test]
async fn a_mission_materializes_grants_for_exactly_its_allowed_tools() {
    let state = app_state("loom-o5-materialize").await;
    let client = EstateClient::new(state.admission(), state.reader());
    mission(&client, "m1", &["search", "calc"], &["crm"], 25).await;

    let mut c = controller(&state, &client);
    settle(&mut c).await;

    let grants = grants_of(&client, "m1").await;
    let tools: Vec<&str> = grants.iter().map(|g| g.spec.tool.as_str()).collect();
    assert_eq!(tools, vec!["calc", "search"], "one grant per allowed tool");
    assert!(
        grants
            .iter()
            .all(|g| g.spec.systems == vec!["crm".to_owned()]),
        "each grant carries the mission's system scope"
    );
    assert_eq!(
        get_mission(&client, "m1")
            .await
            .status
            .expect("status")
            .phase,
        Phase::Ready,
    );
}

#[tokio::test]
async fn shrinking_a_mission_prunes_the_withdrawn_tools_grant() {
    let state = app_state("loom-o5-prune").await;
    let client = EstateClient::new(state.admission(), state.reader());
    mission(&client, "m1", &["search", "calc"], &[], 25).await;

    let mut c = controller(&state, &client);
    settle(&mut c).await;
    assert_eq!(
        grants_of(&client, "m1").await.len(),
        2,
        "both tools granted"
    );

    // Withdraw "calc" from the contract.
    let mut mc = get_mission(&client, "m1").await;
    mc.spec.allowed_tools = vec!["search".to_owned()];
    client
        .update(ResourceObject::MissionContract(mc))
        .await
        .unwrap();
    settle(&mut c).await;

    let grants = grants_of(&client, "m1").await;
    assert_eq!(grants.len(), 1, "the withdrawn tool's grant is pruned");
    assert_eq!(
        grants[0].spec.tool, "search",
        "only the still-allowed tool remains"
    );
}
