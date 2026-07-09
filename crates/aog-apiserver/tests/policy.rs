//! Admission policy: a mutation asserting authority the token lacks is
//! denied (403) with a specific reason; deny-wins holds across composed regimes;
//! a compliant mutation is admitted; a kind with no compliance facts is a no-op.

mod common;

use axum::http::StatusCode;
use common::{BASE, anchor, app_anchored, bundle, header_for, mint_with, send};
use fabric_contracts::{Classification, ComplianceScope};
use serde_json::{Value, json};

fn tenant(name: &str, ceiling: &str, scopes: &[&str]) -> Value {
    json!({
        "api_version": "aog.islandmountain.io/v1",
        "kind": "Tenant",
        "metadata": { "name": name },
        "spec": {
            "display_name": "Test Tenant",
            "ring": 1,
            "classification_ceiling": ceiling,
            "compliance_scopes": scopes,
        },
    })
}

#[tokio::test]
async fn tenant_requiring_unheld_scope_is_forbidden() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k7-hipaa", &signer, None).await;
    // The token holds no compliance scopes; the tenant asserts HIPAA.
    let tok = header_for(&mint_with(&signer, |t| {
        t.compliance_scopes = vec![];
        t.max_data_classification = Classification::Secret;
    }));
    let (status, body) = send(
        &app,
        "POST",
        &format!("{BASE}/Tenant"),
        Some(&tok),
        Some(tenant("acme", "restricted", &["hipaa"])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert!(body["error"].as_str().unwrap().contains("hipaa"));
}

#[tokio::test]
async fn deny_wins_across_regimes() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k7-denywins", &signer, None).await;
    // The token holds HIPAA but not ITAR; the tenant requires both, so the ITAR
    // module denies and deny-wins carries the aggregate.
    let tok = header_for(&mint_with(&signer, |t| {
        t.compliance_scopes = vec![ComplianceScope::Hipaa];
        t.max_data_classification = Classification::Secret;
    }));
    let (status, body) = send(
        &app,
        "POST",
        &format!("{BASE}/Tenant"),
        Some(&tok),
        Some(tenant("acme", "restricted", &["hipaa", "itar_ear"])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert!(
        body["error"].as_str().unwrap().contains("itar"),
        "the ITAR deny must win: {body}"
    );
}

#[tokio::test]
async fn classification_over_authority_is_forbidden() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k7-class", &signer, None).await;
    // Token authority is Internal; the tenant's Secret ceiling exceeds it.
    let tok = header_for(&mint_with(&signer, |t| {
        t.max_data_classification = Classification::Internal;
    }));
    let (status, body) = send(
        &app,
        "POST",
        &format!("{BASE}/Tenant"),
        Some(&tok),
        Some(tenant("acme", "secret", &[])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("classification")
    );
}

#[tokio::test]
async fn compliant_tenant_is_admitted() {
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k7-ok", &signer, None).await;
    // Token holds HIPAA + high classification; the tenant fits within it.
    let tok = header_for(&mint_with(&signer, |t| {
        t.compliance_scopes = vec![ComplianceScope::Hipaa];
        t.max_data_classification = Classification::Secret;
    }));
    let (status, body) = send(
        &app,
        "POST",
        &format!("{BASE}/Tenant"),
        Some(&tok),
        Some(tenant("acme", "restricted", &["hipaa"])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
}

#[tokio::test]
async fn non_compliance_kind_is_admitted() {
    // A PolicyBundle carries no classification/scopes, so policy is a no-op.
    let signer = anchor();
    let app = app_anchored("aog-apiserver-k7-noop", &signer, None).await;
    let tok = header_for(&mint_with(&signer, |t| {
        t.compliance_scopes = vec![];
    }));
    let (status, _) = send(
        &app,
        "POST",
        &format!("{BASE}/PolicyBundle"),
        Some(&tok),
        Some(bundle("pb", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}
