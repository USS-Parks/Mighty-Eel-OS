//! Every admitted mutation emits exactly one hash-chained receipt; a
//! rejected mutation emits none; and the signed evidence pack verifies off-host
//! with the public key alone (tampering breaks it).

mod common;

use aog_apiserver::AppState;
use aog_apiserver::auth::Authenticator;
use aog_apiserver::seal::Sealer;
use aog_store::raft::RaftNode;
use aog_store::{Op, Precondition};
use axum::http::StatusCode;
use common::{BASE, anchor, authed_app_state, bundle, fresh_dir, send};
use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;
use wsf_ledger::verify_pack;

#[tokio::test]
async fn one_receipt_per_mutation_and_pack_verifies_offhost() {
    let (app, state, tok) = authed_app_state("aog-apiserver-k9").await;
    let t = Some(tok.as_str());

    // Three admitted mutations: create, update, delete.
    let (s, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        t,
        Some(bundle("r", 1)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = send(
        &app,
        "PUT",
        &format!("{BASE}/PolicyBundle/r"),
        t,
        Some(bundle("r", 2)),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(&app, "DELETE", &format!("{BASE}/PolicyBundle/r"), t, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // 1:1 — three mutations, three receipts.
    assert_eq!(state.receipts_len(), 3);
    assert_eq!(
        state.audit_intents_len().await.unwrap(),
        3,
        "every mutation has a durable pre-commit audit intent"
    );
    assert_eq!(state.admission().recover_receipts().await.unwrap(), 3);
    assert_eq!(
        state.receipts_len(),
        3,
        "outbox replay is idempotent and emits no duplicate receipts"
    );

    // The signed evidence pack verifies off-host with the public key alone.
    let pack = state.export_receipts("2026-07-04T00:00:00Z").unwrap();
    let public_key = state.receipts_public_key();
    assert_eq!(pack.count, 3);
    assert!(verify_pack(&pack, &MlDsa87Verifier, &public_key));

    // Tampering with a receipt after signing breaks verification.
    let mut tampered = pack.clone();
    tampered.entries[0].receipt = serde_json::json!({ "decision": "forged" });
    assert!(!verify_pack(&tampered, &MlDsa87Verifier, &public_key));

    // A wrong key also fails.
    let (_, other_state, _) = authed_app_state("aog-apiserver-k9-otherkey").await;
    assert!(!verify_pack(
        &pack,
        &MlDsa87Verifier,
        &other_state.receipts_public_key()
    ));
}

#[tokio::test]
async fn rejected_mutation_emits_no_receipt() {
    let (app, state, tok) = authed_app_state("aog-apiserver-k9-reject").await;
    let t = Some(tok.as_str());

    // Structurally invalid (version 0) — rejected before commit.
    let bad = serde_json::json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "PolicyBundle",
        "metadata": { "name": "x" },
        "spec": { "version": 0 },
    });
    let (s, _) = send(&app, "POST", &format!("{BASE}/PolicyBundle"), t, Some(bad)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(
        state.receipts_len(),
        0,
        "a rejected mutation must write no receipt"
    );
    assert_eq!(state.audit_intents_len().await.unwrap(), 0);
}

#[tokio::test]
async fn pending_intent_survives_crash_and_recovers_as_verifiable_evidence() {
    let dir = fresh_dir("aog-apiserver-audit-crash-window");
    let node = RaftNode::bootstrap(1, &dir).await.unwrap();
    let intent_id = "intent-after-commit-before-finalize";
    let pending = serde_json::to_vec(&serde_json::json!({
        "schema": "aog.audit-intent/v1",
        "intent_id": intent_id,
        "correlation_id": "corr-crash",
        "tenant_id": "tenant-loom",
        "subject_hash": "hmac:loom",
        "operation": "aog-create",
        "resource": "PolicyBundle/crash-window",
        "verb": "create",
        "before_digest": null,
        "after_digest": "sha256:planned",
        "planned_op": { "Put": { "key": "PolicyBundle/crash-window" } },
        "created_at": "2026-07-15T00:00:00Z",
    }))
    .unwrap();
    node.write(Op::Put {
        key: format!("AuditOutbox/{intent_id}"),
        value: pending,
        expected: Precondition::Absent,
    })
    .await
    .unwrap();
    node.write(Op::Put {
        key: "PolicyBundle/crash-window".to_owned(),
        value: br#"{"committed":true}"#.to_vec(),
        expected: Precondition::Absent,
    })
    .await
    .unwrap();
    node.shutdown().await.unwrap();

    let signer = anchor();
    let auth = Authenticator::new(signer.public_key().to_vec());
    let state = AppState::start(1, &dir, auth, Sealer::generate().unwrap())
        .await
        .unwrap();

    assert_eq!(state.receipts_len(), 1);
    let pack = state.export_receipts("2026-07-15T00:01:00Z").unwrap();
    assert_eq!(pack.count, 1);
    assert_eq!(
        pack.entries[0].receipt["receipt_id"],
        format!("audit-intent:{intent_id}")
    );
    assert_eq!(pack.entries[0].receipt["mutation_status"], "indeterminate");
    assert!(verify_pack(
        &pack,
        &MlDsa87Verifier,
        &state.receipts_public_key()
    ));
}
