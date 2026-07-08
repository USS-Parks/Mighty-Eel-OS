//! R2 gate — "deleting a Tenant revokes its tokens everywhere + GCs children;
//! no dangling capability." Plus orphan collection by owner reference.
//!
//! The full loop runs live in-process: real apiserver state (admission chain,
//! receipts, front-door authenticator), real controllers on the R1 runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::{Authenticator, TOKEN_HEADER};
use aog_apiserver::seal::Sealer;
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, GarbageCollector, Reconciler, RevocationIndexer,
    SyncStats, TENANT_FINALIZER, TenantTeardown,
};
use aog_estate::{
    CapabilitySpec, Kind, OwnerRef, PlacementSpec, Resource, ResourceObject, TenantSpec,
    VirtualKeySpec, WorkloadKind, WorkloadSpec,
};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use fabric_contracts::{
    Attenuation, Budget, Classification, RevocationStatus, Signature, TrustToken,
};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Mint a signed trust token for `tenant`.
fn mint(signer: &RustCryptoMlDsa87, tenant: &str) -> TrustToken {
    let now = Utc::now();
    let token = TrustToken {
        token_id: format!("tok-{tenant}"),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: tenant.to_owned(),
        subject_id: None,
        subject_hash: format!("hmac:{tenant}"),
        service_identity: Some("aogctl".to_owned()),
        identity_id: None,
        roles: vec![],
        compliance_scopes: vec![],
        allowed_routes: vec![],
        allowed_models: vec![],
        max_data_classification: Classification::Restricted,
        country: None,
        person_type: None,
        offline_mode: false,
        revocation_status: RevocationStatus::Valid,
        budget: None,
        attenuation: Attenuation::default(),
        signature: Signature {
            alg: String::new(),
            key_id: String::new(),
            value: String::new(),
        },
    };
    fabric_token::issue(token, signer).unwrap()
}

fn headers_for(token: &TrustToken) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let value = BASE64.encode(serde_json::to_vec(token).unwrap());
    headers.insert(TOKEN_HEADER, value.parse().unwrap());
    headers
}

async fn estate(dir: &str) -> (AppState, RustCryptoMlDsa87) {
    let signer = RustCryptoMlDsa87::generate("loom-r2-anchor").unwrap();
    let auth = Authenticator::new(signer.public_key().to_vec());
    let state = AppState::bootstrap(1, fresh_dir(dir), auth, Sealer::generate().unwrap())
        .await
        .unwrap();
    (state, signer)
}

fn quiet(stats: SyncStats) -> bool {
    stats.enqueued == 0 && stats.drained == 0 && stats.processed == 0
}

fn idle<R: Reconciler>(controller: &Controller<R>) -> bool {
    controller.queue_len() == 0 && controller.delayed_len() == 0
}

/// Sync all three controllers in rounds until a whole round does nothing.
async fn settle3<A: Reconciler, B: Reconciler, C: Reconciler>(
    a: &mut Controller<A>,
    b: &mut Controller<B>,
    c: &mut Controller<C>,
) {
    for _ in 0..200 {
        let now = Instant::now();
        let (sa, sb, sc) = (
            a.sync(now).await.unwrap(),
            b.sync(now).await.unwrap(),
            c.sync(now).await.unwrap(),
        );
        if quiet(sa) && quiet(sb) && quiet(sc) && idle(a) && idle(b) && idle(c) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("controllers did not settle within 200 rounds");
}

async fn settle1<R: Reconciler>(controller: &mut Controller<R>) {
    for _ in 0..100 {
        let stats = controller.sync(Instant::now()).await.unwrap();
        if quiet(stats) && idle(controller) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("controller did not settle within 100 rounds");
}

#[tokio::test]
async fn deleting_a_tenant_revokes_its_tokens_and_gcs_children() {
    let (state, signer) = estate("loom-r2-tenant-teardown").await;
    let client = EstateClient::new(state.admission(), state.reader());

    // The tenant and its scoped children.
    let tenant = Resource::new(
        "acme",
        TenantSpec {
            display_name: "Acme Health".to_owned(),
            ring: 1,
            classification_ceiling: Classification::Restricted,
            compliance_scopes: vec![],
            subject_hmac_rotation_days: 0,
        },
    );
    client
        .ensure_created(ResourceObject::Tenant(tenant))
        .await
        .unwrap();
    let mut cap = Resource::new(
        "acme-cap",
        CapabilitySpec {
            budget: Budget::default(),
            caveats: vec![],
            allowed_routes: vec![],
            allowed_models: vec![],
            max_classification: Classification::Restricted,
            ttl_seconds: 300,
        },
    );
    cap.metadata.tenant = Some("acme".to_owned());
    client
        .ensure_created(ResourceObject::Capability(cap))
        .await
        .unwrap();
    let mut vkey = Resource::new(
        "acme-key",
        VirtualKeySpec {
            tenant: "acme".to_owned(),
            capability: "acme-cap".to_owned(),
            display_name: String::new(),
        },
    );
    vkey.metadata.tenant = Some("acme".to_owned());
    client
        .ensure_created(ResourceObject::VirtualKey(vkey))
        .await
        .unwrap();

    // The Phase-R controllers on the R1 runtime.
    let mut gc = Controller::new(
        "gc",
        state.informer(""),
        GarbageCollector::new(client.clone()),
        Arc::new(AlwaysLeader),
    );
    let mut teardown = Controller::new(
        "tenant-teardown",
        state.informer("Tenant/"),
        TenantTeardown::new(client.clone()).with_requeue(Duration::from_millis(1)),
        Arc::new(AlwaysLeader),
    );
    let mut indexer = Controller::new(
        "revocation-indexer",
        state.informer("RevocationIntent/"),
        RevocationIndexer::new(client.clone(), state.authenticator().live_revocation()),
        Arc::new(AlwaysLeader),
    );
    settle3(&mut gc, &mut teardown, &mut indexer).await;

    // The live tenant is guarded by the teardown finalizer.
    let guarded = client.get(Kind::Tenant, "acme").await.unwrap().unwrap();
    assert!(
        guarded
            .metadata()
            .finalizers
            .contains(&TENANT_FINALIZER.to_owned()),
        "a live tenant is finalizer-guarded"
    );

    // Before teardown, an acme token passes the front door.
    let acme_token = mint(&signer, "acme");
    assert!(
        state
            .authenticator()
            .authenticate(&headers_for(&acme_token))
            .is_ok(),
        "pre-teardown, the tenant's token authenticates"
    );

    // Deprovision.
    client.delete(Kind::Tenant, "acme").await.unwrap();
    settle3(&mut gc, &mut teardown, &mut indexer).await;

    // GC'd: the tenant and everything scoped to it are gone — no dangling
    // capability, no dangling key.
    assert!(client.get(Kind::Tenant, "acme").await.unwrap().is_none());
    assert!(
        client.list(Kind::Capability).await.unwrap().is_empty(),
        "no dangling capability outlives its tenant"
    );
    assert!(client.list(Kind::VirtualKey).await.unwrap().is_empty());

    // The kill record survives the tenant, acknowledged as propagated.
    let intent = client
        .get(Kind::RevocationIntent, "revoke-tenant-acme")
        .await
        .unwrap()
        .expect("the revocation intent survives the tenant it kills");
    let ResourceObject::RevocationIntent(intent) = intent else {
        panic!("wrong kind");
    };
    let status = intent.status.expect("intent status written");
    assert!(status.propagated, "the kill is live on this replica");

    // Revoked everywhere (kernel leg): the tenant's token now fails the front
    // door; an unrelated tenant's token still passes.
    assert!(
        state
            .authenticator()
            .authenticate(&headers_for(&acme_token))
            .is_err(),
        "post-teardown, the tenant's token is refused"
    );
    let other = mint(&signer, "globex");
    assert!(
        state
            .authenticator()
            .authenticate(&headers_for(&other))
            .is_ok(),
        "an unrelated tenant is untouched"
    );

    // Every teardown step went through admission: receipted, not silent.
    assert!(
        state.receipts_len() >= 8,
        "teardown mutations are receipted (found {})",
        state.receipts_len()
    );
}

#[tokio::test]
async fn orphans_are_collected_by_owner_reference() {
    let (state, _signer) = estate("loom-r2-orphan-gc").await;
    let client = EstateClient::new(state.admission(), state.reader());

    let workload = Resource::new(
        "gw",
        WorkloadSpec {
            workload_kind: WorkloadKind::Gateway,
            replicas: 1,
            ring: 1,
            classification_ceiling: Classification::Restricted,
            image: None,
            command: vec![],
            capability: None,
        },
    );
    client
        .ensure_created(ResourceObject::Workload(workload))
        .await
        .unwrap();
    let owner_uid = client
        .get(Kind::Workload, "gw")
        .await
        .unwrap()
        .unwrap()
        .metadata()
        .uid
        .clone();

    // One placement owned by the live workload, one recorded against a stale
    // incarnation (uid mismatch = its owner no longer exists).
    let mut bound = Resource::new(
        "gw-bound",
        PlacementSpec {
            workload: "gw".to_owned(),
            node: "node-1".to_owned(),
            token_id: String::new(),
        },
    );
    bound.metadata.owner_refs = vec![OwnerRef {
        kind: Kind::Workload,
        name: "gw".to_owned(),
        uid: owner_uid,
    }];
    client
        .ensure_created(ResourceObject::Placement(bound))
        .await
        .unwrap();
    let mut orphaned = Resource::new(
        "gw-stale",
        PlacementSpec {
            workload: "gw".to_owned(),
            node: "node-2".to_owned(),
            token_id: String::new(),
        },
    );
    orphaned.metadata.owner_refs = vec![OwnerRef {
        kind: Kind::Workload,
        name: "gw".to_owned(),
        uid: "uid-of-a-prior-incarnation".to_owned(),
    }];
    client
        .ensure_created(ResourceObject::Placement(orphaned))
        .await
        .unwrap();

    let mut gc = Controller::new(
        "gc",
        state.informer(""),
        GarbageCollector::new(client.clone()),
        Arc::new(AlwaysLeader),
    );
    settle1(&mut gc).await;

    // The stale-incarnation orphan is collected; the live binding survives.
    assert!(
        client
            .get(Kind::Placement, "gw-stale")
            .await
            .unwrap()
            .is_none(),
        "uid-mismatch orphan collected"
    );
    assert!(
        client
            .get(Kind::Placement, "gw-bound")
            .await
            .unwrap()
            .is_some(),
        "a correctly-owned object survives"
    );

    // Deleting the owner cascades to its dependents.
    client.delete(Kind::Workload, "gw").await.unwrap();
    settle1(&mut gc).await;
    assert!(
        client
            .get(Kind::Placement, "gw-bound")
            .await
            .unwrap()
            .is_none(),
        "cascading delete sweeps the owner's dependents"
    );
}
