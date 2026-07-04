//! Integration tests for the + endpoints.
//!
//! Exercises the trust status surface and the compliance
//! management surface through the full axum router so the auth
//! middleware + route dispatch + handler paths are all covered.

use std::sync::Arc;

use axum::body::{self, Body};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;
use mai_api::audit::MemoryAuditWriter;
use mai_api::auth::{ApiKeyStore, AuthState};
use mai_api::config::ServerConfig;
use mai_api::routes::build_router;
use mai_api::state::AppState;
use mai_api::types::ProfileRole;
use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_scheduler::DefaultScheduler;

struct TestVault;

#[async_trait::async_trait]
impl VaultInterface for TestVault {
    async fn load_model_weights(
        &self,
        model_id: &str,
    ) -> Result<Vec<u8>, mai_core::vault::VaultError> {
        Err(mai_core::vault::VaultError::ModelNotFound(
            model_id.to_string(),
        ))
    }
    async fn store_model_package(
        &self,
        _model_id: &str,
        _data: &[u8],
    ) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }
    async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }
    async fn verify_signature(
        &self,
        _data: &[u8],
        _sig: &[u8],
    ) -> Result<bool, mai_core::vault::VaultError> {
        Ok(true)
    }
}

fn build_state_with_admin_key(api_key: &str) -> AppState {
    let scheduler: Arc<dyn mai_scheduler::Scheduler> = Arc::new(DefaultScheduler::new(
        mai_scheduler::SchedulerConfig::default(),
    ));
    let registry = Arc::new(RwLock::new(ModelRegistry::new(Box::new(TestVault))));
    let health = Arc::new(RwLock::new(HealthMonitor::new(HealthConfig::default())));
    let power = Arc::new(RwLock::new(PowerStateMachine::new(PowerConfig::default())));

    let legacy_scheduler =
        mai_core::scheduler::Scheduler::new(mai_core::scheduler::SchedulerConfig::default())
            .unwrap();
    let legacy_scheduler = Arc::new(RwLock::new(legacy_scheduler));
    let hotswap = HotSwapManager::new(legacy_scheduler, registry.clone(), health.clone());
    let hotswap = Arc::new(RwLock::new(hotswap));

    let audit_writer = Arc::new(MemoryAuditWriter::new());
    let config = Arc::new(RwLock::new(ServerConfig::default()));

    let mut store = ApiKeyStore::new();
    store.add_key_raw(
        api_key,
        "admin1".to_string(),
        ProfileRole::Admin,
        Some("Admin One".to_string()),
    );
    let auth = AuthState::with_key_store(store);

    let adapter_manager = AdapterManager::new(FrameworkConfig::default());
    let adapter_manager = Arc::new(Mutex::new(adapter_manager));
    let metrics_collector = Arc::new(mai_scheduler::metrics::MetricsCollector::new(
        mai_scheduler::metrics::MetricsConfig::default(),
    ));

    AppState::new(
        scheduler,
        registry,
        health,
        power,
        hotswap,
        audit_writer,
        config,
        auth,
        adapter_manager,
        metrics_collector,
    )
}

fn auth_request(method: &str, uri: &str, body: Option<&str>, api_key: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-im-auth-token", api_key);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("invalid JSON body: {e}"))
}

// ───: trust endpoints ────────────────────────────────────────

#[tokio::test]
async fn s44_trust_status_returns_air_gapped_by_default() {
    let key = "im-trust-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request("GET", "/v1/trust/status", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // Fresh AppState: never refreshed, default air-gap policy is
    // AirGapped, and the switch wins over the cache-freshness ladder
    // inside LocalTrustCache::evaluate.
    assert_eq!(body["mode"], "air-gapped");
    assert_eq!(body["bundle_version"], Value::Null);
    assert_eq!(body["claim_count"], 0);
    assert_eq!(body["airgap"]["is_air_gapped"], true);
}

#[tokio::test]
async fn s44_trust_bundle_status_shape() {
    let key = "im-bundle-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request("GET", "/v1/trust/bundle_status", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // Default air-gap policy + never-refreshed cache → "air-gapped"
    // wins. `is_emergency_only` only fires on the cache-derived
    // Expired state, so it stays false under physical air-gap.
    assert_eq!(body["connectivity"], "air-gapped");
    assert_eq!(body["is_emergency_only"], false);
    assert_eq!(body["bundle_version"], Value::Null);
}

#[tokio::test]
async fn s44_trust_claims_admin_only_and_empty() {
    let key = "im-claims-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request("GET", "/v1/trust/claims", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["total"], 0);
    assert!(body["claims"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn s44_revocation_status_unknown_for_unseen_claim() {
    let key = "im-revoc-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/trust/revocation_status?claim_id=does-not-exist",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["claim_id"], "does-not-exist");
    assert_eq!(body["status"], "unknown");
}

#[tokio::test]
async fn s44_revocation_status_requires_claim_id() {
    let key = "im-revoc-noid-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/trust/revocation_status",
            None,
            key,
        ))
        .await
        .unwrap();
    // Missing required `claim_id` query → axum's Query extractor short-
    // circuits with 400 Bad Request before our handler runs.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn s44_exchange_token_returns_local_dev_token() {
    let key = "im-exchange-key";
    let router = build_router(build_state_with_admin_key(key));
    let body = serde_json::json!({
        "subject_id": "user-42",
        "tenant_id": "tenant-x",
        "scopes": ["local_only", "view_audit"],
    })
    .to_string();
    let response = router
        .oneshot(auth_request(
            "POST",
            "/v1/auth/exchange_token",
            Some(&body),
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(
        body["token"]
            .as_str()
            .unwrap()
            .starts_with("local-dev.admin1.user-42.")
    );
    assert_eq!(body["mode"], "local-dev");
    assert_eq!(body["tenant_id"], "tenant-x");
    assert_eq!(body["scopes"][0], "local_only");
    assert!(body["expires_at_secs"].as_u64().unwrap() > body["issued_at_secs"].as_u64().unwrap());
}

// ─── compliance endpoints ────────────────────────────────────

#[tokio::test]
async fn s44_compliance_status_lists_standard_template_modules() {
    let key = "im-status-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request("GET", "/v1/compliance/status", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // Standard template ships HIPAA-only enabled.
    let modules = body["modules"].as_array().unwrap();
    assert!(
        !modules.is_empty(),
        "Standard template has at least one module"
    );
    let hipaa = modules.iter().find(|m| m["module"] == "hipaa").unwrap();
    assert_eq!(hipaa["enabled"], true);
    assert_eq!(body["reload_count"], 0);
    assert!(body["audit_integrity"]["entry_count"].is_u64());
}

#[tokio::test]
async fn s44_compliance_policies_list_and_get() {
    let key = "im-policies-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .clone()
        .oneshot(auth_request("GET", "/v1/compliance/policies", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(!body["modules"].as_array().unwrap().is_empty());

    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/compliance/policies/hipaa",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["module"], "hipaa");
}

#[tokio::test]
async fn s44_compliance_apply_template_swaps_active_policy() {
    let key = "im-template-key";
    let router = build_router(build_state_with_admin_key(key));
    let body = serde_json::json!({"template": "defense"}).to_string();
    let response = router
        .clone()
        .oneshot(auth_request(
            "POST",
            "/v1/compliance/policies/template",
            Some(&body),
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router
        .oneshot(auth_request("GET", "/v1/compliance/status", None, key))
        .await
        .unwrap();
    let body = json_body(response).await;
    let modules = body["modules"].as_array().unwrap();
    let itar = modules.iter().find(|m| m["module"] == "itar").unwrap();
    assert_eq!(itar["enabled"], true);
}

#[tokio::test]
async fn s44_compliance_module_toggle_emits_changed_flag() {
    let key = "im-toggle-key";
    let router = build_router(build_state_with_admin_key(key));
    let body = serde_json::json!({"enabled": false}).to_string();
    let response = router
        .oneshot(auth_request(
            "PUT",
            "/v1/compliance/policies/hipaa",
            Some(&body),
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["module"], "hipaa");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["changed"], true);
}

#[tokio::test]
async fn s44_compliance_unknown_module_returns_validation_error() {
    let key = "im-unknown-mod-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/compliance/policies/phi",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(response).await;
    assert_eq!(body["error"]["code"], "MAI-1002");
}

#[tokio::test]
async fn s44_compliance_audit_query_returns_empty_list_initially() {
    let key = "im-audit-q-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/compliance/audit?limit=10",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn s44_compliance_audit_integrity_starts_unknown() {
    let key = "im-audit-int-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/compliance/audit/integrity",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["entry_count"], 0);
    assert_eq!(body["last_verify"], "unknown");
}

#[tokio::test]
async fn s44_compliance_audit_verify_succeeds_on_empty_chain() {
    let key = "im-audit-verify-key";
    let router = build_router(build_state_with_admin_key(key));
    let response = router
        .oneshot(auth_request(
            "GET",
            "/v1/compliance/audit/verify",
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["verified"], true);
}

#[tokio::test]
async fn s44_compliance_reports_list_and_generate_round_trip() {
    let key = "im-reports-key";
    let router = build_router(build_state_with_admin_key(key));

    // List starts empty.
    let response = router
        .clone()
        .oneshot(auth_request("GET", "/v1/compliance/reports", None, key))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["total"], 0);

    // Generate a SystemActivity report over the empty audit window.
    let body = serde_json::json!({
        "report_type": "system_activity",
        "from_unix_nanos": 0,
        "to_unix_nanos": 1_000_000_000u64,
        "tenant": "acme",
        "format": "json",
    })
    .to_string();
    let response = router
        .clone()
        .oneshot(auth_request(
            "POST",
            "/v1/compliance/reports/generate",
            Some(&body),
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["report_type"], "system_activity");
    assert_eq!(body["output_format"], "json");
    assert_eq!(body["tenant"], "acme");
    assert_eq!(body["status"], "complete");
    assert!(body["content_hash_hex"].as_str().is_some());

    // List now has one record.
    let response = router
        .clone()
        .oneshot(auth_request("GET", "/v1/compliance/reports", None, key))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(body["total"], 1);

    // Single-record lookup works.
    let response = router
        .clone()
        .oneshot(auth_request(
            "GET",
            &format!("/v1/compliance/reports/{id}"),
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Download yields bytes with the right content type.
    let response = router
        .clone()
        .oneshot(auth_request(
            "GET",
            &format!("/v1/compliance/reports/{id}/download"),
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
    );

    // Delete succeeds; subsequent GET returns 404.
    let response = router
        .clone()
        .oneshot(auth_request(
            "DELETE",
            &format!("/v1/compliance/reports/{id}"),
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router
        .oneshot(auth_request(
            "GET",
            &format!("/v1/compliance/reports/{id}"),
            None,
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn s44_compliance_generate_rejects_inverted_date_range() {
    let key = "im-bad-range-key";
    let router = build_router(build_state_with_admin_key(key));
    let body = serde_json::json!({
        "report_type": "system_activity",
        "from_unix_nanos": 1_000,
        "to_unix_nanos": 500,
    })
    .to_string();
    let response = router
        .oneshot(auth_request(
            "POST",
            "/v1/compliance/reports/generate",
            Some(&body),
            key,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn s44_compliance_endpoints_require_auth() {
    let key = "im-no-auth-key";
    let router = build_router(build_state_with_admin_key(key));
    // No x-im-auth-token header → unauthorised.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/compliance/status")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
