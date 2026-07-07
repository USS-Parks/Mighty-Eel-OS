//! L1/L2 gate (offline) — receipt queries are authenticated and mandatorily
//! tenant-scoped (AF-007). A principal sees only its own tenant's receipts; a
//! cross-tenant identifier query returns no rows and no existence oracle. The
//! one exception is a server-enrolled **global auditor** (L2), who may read
//! across tenants and export the signed evidence pack (L4) — everyone else
//! gets 403 on export.
//!
//! Runs with no OpenBao: the ledger is seeded directly with receipts for two
//! tenants, and the router is driven over HTTP with a dev principal per tenant.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::{Arc, Mutex};

use fabric_crypto::Signer;
use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};
use reqwest::Client;
use serde_json::{Value, json};
use wsf_api::AppState;
use wsf_api::audit::StaticAuditors;
use wsf_api::auth::LocalDevAuthenticator;
use wsf_api::grants::StaticGrants;
use wsf_api::policy::StaticTenantPolicies;
use wsf_bridge::{BridgeConfig, OpenBaoAuth, OpenBaoConfig, TrustBridge};
use wsf_broker::{AwsStsBroker, BrokerConfig};
use wsf_ledger::{EvidencePack, Ledger};
use wsf_seal::{SealService, SealServiceConfig};

fn unused() -> OpenBaoAuth {
    OpenBaoAuth::new(OpenBaoConfig::new("http://127.0.0.1:1", "r", "s")).unwrap()
}

/// A ledger pre-seeded with one issuance receipt per tenant.
fn seeded_ledger() -> Arc<Mutex<Ledger>> {
    let ledger = Ledger::new(Arc::new(RustCryptoMlDsa87::generate("l-ledger").unwrap()));
    let ledger = Arc::new(Mutex::new(ledger));
    {
        let mut l = ledger.lock().unwrap();
        l.ingest(
            "wsf-bridge",
            json!({ "kind": "issuance_decision", "decision": "allow",
                    "tenant_id": "tenant-a", "token_id": "tok-a", "principal_id": "p-a" }),
        )
        .unwrap();
        l.ingest(
            "wsf-bridge",
            json!({ "kind": "issuance_decision", "decision": "allow",
                    "tenant_id": "tenant-b", "token_id": "tok-b", "principal_id": "p-b" }),
        )
        .unwrap();
    }
    ledger
}

async fn spawn_as(tenant: &str, ledger: Arc<Mutex<Ledger>>) -> String {
    spawn_with(tenant, ledger, StaticAuditors::none()).await
}

async fn spawn_with(tenant: &str, ledger: Arc<Mutex<Ledger>>, auditors: StaticAuditors) -> String {
    let anchor = RustCryptoMlDsa87::generate("l-anchor")
        .unwrap()
        .public_key()
        .to_vec();
    let state = AppState {
        bridge: Arc::new(TrustBridge::new(
            unused(),
            Arc::new(RustCryptoMlDsa87::generate("l-bridge").unwrap()),
            BridgeConfig::new("l", vec![1u8; 32]),
        )),
        broker: Arc::new(AwsStsBroker::new(
            unused(),
            Client::new(),
            BrokerConfig::new("us-east-1", "http://127.0.0.1:1", "kv/data/broker/x"),
        )),
        seal: Arc::new(SealService::new(
            unused(),
            Arc::new(RustCryptoMlDsa87::generate("l-seal").unwrap()),
            SealServiceConfig {
                transit_key: "x".into(),
                token_public_key: anchor.clone(),
            },
        )),
        ledger,
        token_public_key: Arc::new(anchor),
        auth: Arc::new(LocalDevAuthenticator::for_wsf(tenant)),
        policy: Arc::new(StaticTenantPolicies::single_dev(tenant, &["user"])),
        grants: Arc::new(StaticGrants::new()),
        auditors: Arc::new(auditors),
    };
    let app = wsf_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn receipts(base: &str, query: &str) -> Vec<Value> {
    let resp: Value = Client::new()
        .get(format!("{base}/v1/receipts{query}"))
        .send()
        .await
        .expect("receipts req")
        .json()
        .await
        .expect("receipts json");
    resp["entries"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn receipts_are_tenant_scoped_with_no_cross_tenant_oracle() {
    let ledger = seeded_ledger();

    // Tenant A's principal sees only tenant A's receipt.
    let base_a = spawn_as("tenant-a", ledger.clone()).await;
    let all_a = receipts(&base_a, "").await;
    assert_eq!(all_a.len(), 1, "tenant A sees exactly its own receipt");
    assert_eq!(all_a[0]["receipt"]["tenant_id"], "tenant-a");

    // Querying tenant B's token id from A's session returns nothing — no
    // existence oracle for another tenant's identifiers.
    let cross = receipts(&base_a, "?field=token_id&value=tok-b").await;
    assert!(
        cross.is_empty(),
        "cross-tenant identifier query returns no rows"
    );

    // A's own token id query returns A's receipt.
    let own = receipts(&base_a, "?field=token_id&value=tok-a").await;
    assert_eq!(own.len(), 1, "own-tenant identifier query works");

    // Tenant B's principal symmetrically sees only tenant B's receipt.
    let base_b = spawn_as("tenant-b", ledger.clone()).await;
    let all_b = receipts(&base_b, "").await;
    assert_eq!(all_b.len(), 1);
    assert_eq!(all_b[0]["receipt"]["tenant_id"], "tenant-b");

    println!("L1/L2 gate PASSED: receipts authenticated + tenant-scoped; no cross-tenant oracle");
}

#[tokio::test]
async fn global_auditor_reads_across_tenants_and_exports_a_verifiable_pack() {
    let ledger = seeded_ledger();
    let ledger_public_key = ledger.lock().unwrap().public_key().to_vec();

    // The dev principal_id is "local-dev"; enroll it as a global auditor on
    // one plane only. Enrollment is server-side — nothing in the request.
    let auditor_base = spawn_with(
        "tenant-a",
        ledger.clone(),
        StaticAuditors::none().with("local-dev"),
    )
    .await;
    let plain_base = spawn_as("tenant-a", ledger.clone()).await;

    // L2: the auditor sees BOTH tenants' receipts; the plain principal one.
    let audited = receipts(&auditor_base, "").await;
    assert_eq!(audited.len(), 2, "auditor sees both tenants");
    let plain = receipts(&plain_base, "").await;
    assert_eq!(plain.len(), 1, "non-auditor stays tenant-scoped");

    // L4: the auditor exports a signed evidence pack that verifies OFFLINE
    // with the ledger's public key alone.
    let pack: EvidencePack = Client::new()
        .get(format!("{auditor_base}/v1/receipts/export"))
        .send()
        .await
        .expect("export req")
        .error_for_status()
        .expect("export 200")
        .json()
        .await
        .expect("pack json");
    assert_eq!(pack.count, 2);
    assert!(
        wsf_ledger::verify_pack(&pack, &MlDsa87Verifier, &ledger_public_key),
        "exported pack verifies offline"
    );

    // Tampering any entry breaks the pack signature.
    let mut tampered = pack.clone();
    tampered.entries[0].receipt["decision"] = json!("deny");
    assert!(
        !wsf_ledger::verify_pack(&tampered, &MlDsa87Verifier, &ledger_public_key),
        "tampered pack must not verify"
    );

    // The non-auditor is refused the export outright.
    let denied = Client::new()
        .get(format!("{plain_base}/v1/receipts/export"))
        .send()
        .await
        .expect("export req");
    assert_eq!(denied.status().as_u16(), 403, "non-auditor export is 403");

    println!(
        "L2/L4 gate PASSED: auditor-only cross-tenant reads + signed export verifies offline; tamper + non-auditor refused"
    );
}
