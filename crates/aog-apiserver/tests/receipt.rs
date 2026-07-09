//! Every admitted mutation emits exactly one hash-chained receipt; a
//! rejected mutation emits none; and the signed evidence pack verifies off-host
//! with the public key alone (tampering breaks it).

mod common;

use axum::http::StatusCode;
use common::{BASE, authed_app_state, bundle, send};
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
}
