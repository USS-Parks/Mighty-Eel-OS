//! O6 gate — revoking a `ToolGrant` halts the tool mid-run on **every** proxy.
//! The controller compiles the live grants into a signed set on a shared channel;
//! two independent proxy edges poll it and enforce per call. Deleting a grant
//! republishes a newer set without its tool, and both proxies deny the tool's
//! next call. Anti-rollback (a replayed older set is refused) is proven in the
//! `toolgrants.rs` unit tests.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EdgeGrantCache, EstateClient, GrantStore, MemGrantStore, Reconciler,
    SyncStats, ToolGrantController,
};
use aog_estate::{Kind, Resource, ResourceObject, ToolGrantSpec};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn app_state(dir: &str) -> AppState {
    let anchor = RustCryptoMlDsa87::generate("loom-o6-state").unwrap();
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

async fn tool_grant(client: &EstateClient, name: &str, tool: &str, systems: &[&str]) {
    client
        .ensure_created(ResourceObject::ToolGrant(Resource::new(
            name,
            ToolGrantSpec {
                tool: tool.to_owned(),
                systems: systems.iter().map(|s| (*s).to_owned()).collect(),
                requires_approval: false,
                credential_ref: None,
            },
        )))
        .await
        .unwrap();
}

#[tokio::test]
async fn revoking_a_grant_halts_the_tool_on_every_proxy() {
    let state = app_state("loom-o6-revoke").await;
    let client = EstateClient::new(state.admission(), state.reader());
    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-o6-anchor").unwrap());
    let signer: Arc<dyn Signer> = anchor.clone();
    let store = Arc::new(MemGrantStore::new());

    tool_grant(&client, "g-search", "search", &["crm"]).await;
    tool_grant(&client, "g-calc", "calc", &[]).await;

    let reconciler = ToolGrantController::new(client.clone(), Arc::clone(&store), signer);
    let mut controller = Controller::new(
        "toolgrants",
        state.informer("ToolGrant/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle(&mut controller).await;

    // Two independent proxies poll the same channel and verify with the public
    // key alone.
    let public_key = anchor.public_key().to_vec();
    let mut proxy_a = EdgeGrantCache::new(public_key.clone());
    let mut proxy_b = EdgeGrantCache::new(public_key);
    let set = store
        .fetch()
        .await
        .unwrap()
        .expect("a grant set is published");
    proxy_a.accept(set.clone()).expect("proxy A accepts");
    proxy_b.accept(set).expect("proxy B accepts");
    assert!(
        proxy_a.allows("", "calc") && proxy_b.allows("", "calc"),
        "calc is granted on both proxies"
    );
    assert!(proxy_a.allows("", "search") && proxy_b.allows("", "search"));

    // Revoke calc by deleting its ToolGrant; the controller republishes.
    client.delete(Kind::ToolGrant, "g-calc").await.unwrap();
    settle(&mut controller).await;

    let set2 = store
        .fetch()
        .await
        .unwrap()
        .expect("a newer grant set is published");
    assert!(
        set2.version > 1,
        "the republished set advances the version (anti-rollback)"
    );
    proxy_a
        .accept(set2.clone())
        .expect("proxy A pulls newer set");
    proxy_b.accept(set2).expect("proxy B pulls newer set");
    assert!(
        !proxy_a.allows("", "calc") && !proxy_b.allows("", "calc"),
        "the revoked tool is halted on every proxy"
    );
    assert!(
        proxy_a.allows("", "search") && proxy_b.allows("", "search"),
        "a still-granted tool keeps working"
    );
}
