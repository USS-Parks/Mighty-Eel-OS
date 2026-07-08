//! R4 gate — "disabling a ring key makes its envelopes unreadable + halts
//! ring workloads", against a **live** OpenBao (A3.2 no-mock-only closure).
//!
//! Env-gated on `WSF_OPENBAO_ADDR` like the W1/R3 live suites; returns cleanly
//! otherwise. Real crypto end to end: an envelope is sealed through
//! `wsf-seal` under the ring's Transit key, readable while the ring is live,
//! and **provably unreadable** the moment a `RevocationIntent` darkens the
//! ring — the key that wrapped its data key no longer exists.
#![allow(clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_controller::transit::{TransitAdmin, ring_key_name};
use aog_controller::{
    AlwaysLeader, Controller, EstateClient, Reconciler, SyncStats, TrustRingController,
};
use aog_estate::{
    Kind, Phase, Resource, ResourceObject, RevocationIntentSpec, RevocationTarget, TrustRingSpec,
    WorkloadKind, WorkloadSpec,
};
use chrono::Utc;
use fabric_contracts::{Attenuation, Classification, RevocationStatus, Signature, TrustToken};
use fabric_crypto::Signer;
use fabric_crypto::providers::RustCryptoMlDsa87;
use reqwest::{Client, Method};
use serde_json::json;
use wsf_bridge::{OpenBaoAuth, OpenBaoConfig};
use wsf_seal::{LabelSpec, SealRequest, SealService, SealServiceConfig, UnsealRequest};

const ROLE: &str = "loom-r4-ring";

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

/// Bootstrap: mount Transit (root), and an AppRole with key-admin +
/// encrypt/decrypt capabilities.
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
        "sys/mounts/transit",
        Some(json!({"type":"transit"})),
    )
    .await;

    let policy = r#"
path "transit/keys"      { capabilities = ["list"] }
path "transit/keys/*"    { capabilities = ["create", "read", "update", "delete"] }
path "transit/encrypt/*" { capabilities = ["create", "update"] }
path "transit/decrypt/*" { capabilities = ["create", "update"] }
"#;
    bao(
        c,
        addr,
        tok,
        Method::PUT,
        "sys/policies/acl/loom-r4-ring",
        Some(json!({ "policy": policy })),
    )
    .await;
    bao(
        c,
        addr,
        tok,
        Method::POST,
        &format!("auth/approle/role/{ROLE}"),
        Some(json!({"token_policies":"default,loom-r4-ring","token_ttl":"15m"})),
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

fn mint(signer: &RustCryptoMlDsa87, tenant: &str, id: &str) -> TrustToken {
    let now = Utc::now();
    let token = TrustToken {
        token_id: id.to_owned(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
        issuer: "wsf-bridge".to_owned(),
        trust_bundle_version: "2026.07.loom".to_owned(),
        tenant_id: tenant.to_owned(),
        subject_id: None,
        subject_hash: format!("hmac:{tenant}:{id}"),
        service_identity: Some("wsf-seal".to_owned()),
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

fn quiet(stats: SyncStats) -> bool {
    stats.enqueued == 0 && stats.drained == 0 && stats.processed == 0
}

fn idle<R: Reconciler>(c: &Controller<R>) -> bool {
    c.queue_len() == 0 && c.delayed_len() == 0
}

async fn settle2<A: Reconciler, B: Reconciler>(a: &mut Controller<A>, b: &mut Controller<B>) {
    for _ in 0..200 {
        let now = Instant::now();
        let (sa, sb) = (a.sync(now).await.unwrap(), b.sync(now).await.unwrap());
        if quiet(sa) && quiet(sb) && idle(a) && idle(b) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("controllers did not settle within 200 rounds");
}

#[tokio::test]
async fn darkening_a_ring_kills_its_envelopes_and_halts_its_workloads() {
    let Some(addr) = openbao_addr() else {
        eprintln!(
            "SKIP darkening_a_ring_kills_its_envelopes_and_halts_its_workloads: WSF_OPENBAO_ADDR unset (R4 live gate)"
        );
        return;
    };
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let (role_id, secret_id) = bootstrap(&http, &addr, &root_token()).await;

    let anchor = Arc::new(RustCryptoMlDsa87::generate("loom-r4-anchor").unwrap());
    let state = AppState::bootstrap(
        1,
        fresh_dir("loom-r4-live"),
        Authenticator::new(anchor.public_key().to_vec()),
        Sealer::generate().unwrap(),
    )
    .await
    .unwrap();
    let client = EstateClient::new(state.admission(), state.reader());
    let transit = Arc::new(
        TransitAdmin::new(
            OpenBaoAuth::new(OpenBaoConfig::new(
                &addr,
                role_id.clone(),
                secret_id.clone(),
            ))
            .unwrap(),
        )
        .unwrap(),
    );

    // A Ring-3 trust ring and a Ring-3 workload.
    client
        .ensure_created(ResourceObject::TrustRing(Resource::new(
            "ring-three",
            TrustRingSpec {
                ring: 3,
                transit_key: "ring3-seal".to_owned(),
                attestation: aog_estate::AttestationProfile::default(),
            },
        )))
        .await
        .unwrap();
    client
        .ensure_created(ResourceObject::Workload(Resource::new(
            "phi-inference",
            WorkloadSpec {
                workload_kind: WorkloadKind::Inference,
                replicas: 1,
                ring: 3,
                classification_ceiling: Classification::Restricted,
                image: None,
                command: vec![],
                capability: None,
            },
        )))
        .await
        .unwrap();

    // The ring controller, woken by both TrustRing and RevocationIntent edits.
    let reconciler = TrustRingController::new(client.clone(), Arc::clone(&transit));
    let mut rings = Controller::new(
        "trustring",
        state.informer("TrustRing/"),
        reconciler.clone(),
        Arc::new(AlwaysLeader),
    );
    let mut ring_intents = Controller::new(
        "trustring-intents",
        state.informer("RevocationIntent/"),
        reconciler,
        Arc::new(AlwaysLeader),
    );
    settle2(&mut rings, &mut ring_intents).await;

    // ── Live: the per-ring Transit key exists and status reports it.
    let key = ring_key_name(3);
    assert_eq!(
        transit.key_version(&key).await.unwrap(),
        Some(1),
        "ring key provisioned in live Transit"
    );
    let Some(ResourceObject::TrustRing(ring)) =
        client.get(Kind::TrustRing, "ring-three").await.unwrap()
    else {
        panic!("ring missing");
    };
    let status = ring.status.clone().expect("ring status written");
    assert_eq!(status.phase, Phase::Ready);
    assert!(!status.dark);
    assert_eq!(status.key_version, Some(1));

    // ── Seal an envelope under the ring key; readable while the ring is live.
    let seal_service = SealService::new(
        OpenBaoAuth::new(OpenBaoConfig::new(&addr, role_id, secret_id)).unwrap(),
        Arc::clone(&anchor) as Arc<dyn Signer>,
        SealServiceConfig {
            transit_key: key.clone(),
            token_public_key: anchor.public_key().to_vec(),
        },
    );
    let token = mint(&anchor, "acme", "tok-seal");
    let envelope = seal_service
        .seal(
            SealRequest {
                token: token.clone(),
                plaintext: b"ring-3 classified payload".to_vec(),
                label: LabelSpec {
                    classification: Classification::Restricted,
                    compliance_scopes: vec![],
                    origin: "loom-r4-live".to_owned(),
                    permitted_ops: vec![],
                    permitted_destinations: vec![],
                    detected_entities: vec![],
                },
                envelope_id: "env-ring3".to_owned(),
            },
            Utc::now(),
        )
        .await
        .expect("seal under the live ring key");
    let plaintext = seal_service
        .unseal(
            UnsealRequest {
                token: token.clone(),
                envelope: envelope.clone(),
            },
            Utc::now(),
        )
        .await
        .expect("unseal while the ring is live");
    assert_eq!(plaintext, b"ring-3 classified payload");

    // ── Darken the ring: one declarative intent.
    client
        .ensure_created(ResourceObject::RevocationIntent(Resource::new(
            "darken-ring-three",
            RevocationIntentSpec {
                target: RevocationTarget::Ring(3),
                reason: "ring 3 compromise drill".to_owned(),
            },
        )))
        .await
        .unwrap();
    settle2(&mut rings, &mut ring_intents).await;

    // The key is gone from Transit…
    assert_eq!(
        transit.key_version(&key).await.unwrap(),
        None,
        "ring key disabled"
    );
    // …including the per-tenant derivative the seal actually wrapped under
    // (E2 namespacing): the dark switch covers the whole key family.
    assert_eq!(
        transit.key_version(&format!("{key}-acme")).await.unwrap(),
        None,
        "per-tenant ring key disabled with the family"
    );
    // …so the envelope is unreadable: its wrapped data key cannot decrypt.
    assert!(
        seal_service
            .unseal(UnsealRequest { token, envelope }, Utc::now(),)
            .await
            .is_err(),
        "a dark ring's envelopes stop unsealing"
    );

    // The ring reports dark…
    let Some(ResourceObject::TrustRing(ring)) =
        client.get(Kind::TrustRing, "ring-three").await.unwrap()
    else {
        panic!("ring missing");
    };
    let status = ring.status.expect("ring status");
    assert!(status.dark);
    assert_eq!(status.phase, Phase::Degraded);

    // …its workloads are halted…
    let Some(ResourceObject::Workload(workload)) =
        client.get(Kind::Workload, "phi-inference").await.unwrap()
    else {
        panic!("workload missing");
    };
    let wl_status = workload.status.expect("workload status");
    assert_eq!(wl_status.phase, Phase::Failed, "ring workloads halted");
    assert_eq!(wl_status.ready_replicas, 0);

    // …and the kill is acknowledged, not asserted.
    let Some(ResourceObject::RevocationIntent(intent)) = client
        .get(Kind::RevocationIntent, "darken-ring-three")
        .await
        .unwrap()
    else {
        panic!("intent missing");
    };
    assert!(intent.status.expect("intent status").propagated);
}
