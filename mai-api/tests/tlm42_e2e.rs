//! TLM-4.2 E2E: live credential rotation against running OpenBao.
//!
//! Skipped unless `MAI_OPENBAO_SECRET_ID` is set and OpenBao is reachable.
//! Run manually:
//!   $env:MAI_OPENBAO_SECRET_ID = '<live secret>'
//!   cargo test -p mai-api --test tlm42_e2e -- --nocapture

use mai_api::openbao_client::{OpenBaoBridgeClient, OpenBaoBridgeConfig, OpenbaoHealth};
use mai_compliance::trust_cache::{LocalTrustCache, SnapshotStatus};
use std::sync::Arc;
use tokio::sync::RwLock;

fn has_live_openbao() -> bool {
    std::env::var("MAI_OPENBAO_SECRET_ID").is_ok()
}

#[tokio::test]
async fn tlm42_credential_rotation_issues_claims_before_and_after() {
    if !has_live_openbao() {
        eprintln!("[TLM-4.2] SKIP: MAI_OPENBAO_SECRET_ID not set");
        return;
    }

    let config = OpenBaoBridgeConfig::staging();
    let bridge = OpenBaoBridgeClient::new(config);
    let bridge_lock = Arc::new(RwLock::new(Some(bridge.clone())));

    let claim = bridge
        .issue_claim(
            "alice-tlm42",
            "tribal-health-demo",
            vec!["clinician".into()],
            None,
        )
        .await
        .expect("pre-rotation claim must succeed");
    assert_eq!(claim.revocation_status, "valid");
    eprintln!(
        "[TLM-4.2] pre-rotation claim: {} (status: {})",
        claim.claim_id, claim.revocation_status
    );

    let current_secret = std::env::var("MAI_OPENBAO_SECRET_ID").unwrap();
    bridge
        .rotate_credential(&bridge_lock, &current_secret)
        .await
        .expect("rotate_credential must succeed");
    eprintln!("[TLM-4.2] credential rotated in-place");

    let guard = bridge_lock.read().await;
    let new_bridge = guard
        .as_ref()
        .expect("bridge must be present after rotation");
    let claim2 = new_bridge
        .issue_claim(
            "bob-tlm42",
            "tribal-health-demo",
            vec!["viewer".into()],
            None,
        )
        .await
        .expect("post-rotation claim must succeed");
    assert_eq!(claim2.revocation_status, "valid");
    eprintln!(
        "[TLM-4.2] post-rotation claim: {} (status: {})",
        claim2.claim_id, claim2.revocation_status
    );

    drop(guard);
    eprintln!("[TLM-4.2] PASS: rotation mechanism works");
}

#[tokio::test]
async fn tlm42_rotation_preserves_health_check() {
    if !has_live_openbao() {
        eprintln!("[TLM-4.2] SKIP: MAI_OPENBAO_SECRET_ID not set");
        return;
    }

    let config = OpenBaoBridgeConfig::staging();
    let bridge = OpenBaoBridgeClient::new(config);
    let bridge_lock = Arc::new(RwLock::new(Some(bridge.clone())));

    let health = bridge.health_check().await;
    assert!(health.reachable, "OpenBao must be reachable");
    eprintln!(
        "[TLM-4.2] pre-rotation health: reachable={}",
        health.reachable
    );

    let current_secret = std::env::var("MAI_OPENBAO_SECRET_ID").unwrap();
    bridge
        .rotate_credential(&bridge_lock, &current_secret)
        .await
        .expect("rotation must succeed");

    let guard = bridge_lock.read().await;
    let new_bridge = guard.as_ref().unwrap();
    let health2 = new_bridge.health_check().await;
    assert!(health2.reachable, "post-rotation reachable");
    eprintln!(
        "[TLM-4.2] post-rotation health: reachable={} latency_ms={}",
        health2.reachable, health2.latency_ms
    );
    drop(guard);
    eprintln!("[TLM-4.2] PASS: health preserved across rotation");
}

#[tokio::test]
async fn tlm42_rotation_can_initialize_empty_lock() {
    let config = OpenBaoBridgeConfig::staging();
    let bridge = OpenBaoBridgeClient::new(config);
    let empty_lock: Arc<RwLock<Option<OpenBaoBridgeClient>>> = Arc::new(RwLock::new(None));

    let result = bridge.rotate_credential(&empty_lock, "dummy-secret").await;
    assert!(
        result.is_ok(),
        "rotation writes to lock regardless of previous state"
    );
    assert!(empty_lock.read().await.is_some(), "lock now populated");
    eprintln!("[TLM-4.2] PASS: rotation can initialize an empty lock");
}
