//! K8 — the mutate stage seals flagged spec fields at rest (unreadable in the
//! store) and authorizes the object with a child token scoped to the action; the
//! scoped child is a strict subset of the parent.

mod common;

use axum::http::StatusCode;
use common::{BASE, anchor, app_anchored, header_for, mint, mint_with, send};
use fabric_contracts::{Budget, Classification};
use fabric_crypto::Signer;
use fabric_crypto::providers::MlDsa87Verifier;
use serde_json::json;

#[tokio::test]
async fn flagged_field_is_sealed_at_rest() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k8-seal", &signer, None).await;
    let tok = header_for(&mint(&signer));
    let secret = "super-secret-transit-key-material";
    let ring = json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "TrustRing",
        "metadata": { "name": "ring1" },
        "spec": { "ring": 1, "transit_key": secret },
    });
    let (status, created) = send(
        &app,
        "POST",
        &format!("{BASE}/TrustRing"),
        Some(&tok),
        Some(ring),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");

    // The plaintext transit_key is gone; a placeholder stands in its place.
    assert_ne!(created["spec"]["transit_key"], secret);
    assert_eq!(created["spec"]["transit_key"], "sealed:wsf-envelope");
    // The sealed ciphertext lives in an annotation (an AES-256-GCM envelope).
    let sealed = created["metadata"]["annotations"]["wsf.io/sealed.transit_key"]
        .as_str()
        .unwrap();
    assert!(sealed.contains("ciphertext"));
    assert!(
        !sealed.contains(secret),
        "the plaintext must not appear in the sealed blob"
    );

    // Read-back from the store shows the sealed form, never the plaintext.
    let (status, got) = send(
        &app,
        "GET",
        &format!("{BASE}/TrustRing/ring1"),
        Some(&tok),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["spec"]["transit_key"], "sealed:wsf-envelope");
    assert!(
        !got.to_string().contains(secret),
        "the store must not surface the plaintext"
    );

    // The object is authorized by a child token scoped to this action.
    assert!(
        created["metadata"]["token_ref"]["token_id"]
            .as_str()
            .unwrap()
            .starts_with("child:")
    );
}

#[tokio::test]
async fn scoped_child_is_a_subset_of_the_parent() {
    let signer = anchor();
    let parent = mint_with(&signer, |t| {
        t.max_data_classification = Classification::Secret;
        t.budget = Some(Budget {
            token_cap: 1000,
            tokens_spent: 200,
            usd_cap_cents: 500,
            usd_spent_cents: 100,
            tool_call_cap: 10,
            tool_calls_spent: 4,
        });
    });
    let child = aog_apiserver::seal::scoped_child_token(
        &parent,
        Some(Classification::Internal),
        "child:Tenant/acme",
        &signer,
        signer.public_key(),
    )
    .unwrap();

    // Bound to the parent, narrowed on classification, budget <= parent remaining.
    assert_eq!(
        child.attenuation.parent_id.as_deref(),
        Some(parent.token_id.as_str())
    );
    assert_eq!(child.max_data_classification, Classification::Internal);
    let cb = child.budget.as_ref().unwrap();
    assert_eq!(cb.token_cap, 800); // 1000 - 200
    assert_eq!(cb.usd_cap_cents, 400); // 500 - 100
    assert_eq!(cb.tool_call_cap, 6); // 10 - 4
    // And it verifies under the minting signer's key (a valid attenuation).
    fabric_token::verify(&child, &MlDsa87Verifier, signer.public_key()).unwrap();
}
