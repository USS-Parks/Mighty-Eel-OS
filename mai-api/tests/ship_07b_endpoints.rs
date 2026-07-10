//! Acceptance tests.
//!
//! Drives the two new public surfaces end-to-end through axum:
//!
//! 1. `GET /v1/system/production-readiness` — admin-only readiness
//!    report. 422 without a ship profile, 200 + JSON with one, 403 for
//!    non-admin callers.
//! 2. `POST /v1/auth/exchange_token` — profile-aware token exchange.
//!    Mints synthetic on `LocalDevSynthetic`, 503 on `OpenBaoBridge`
//!    until the bridge client lands, 410 on `Disabled`.
//!
//! Tests build a minimal `AppState` directly (no socket bind), drive
//! it via `tower::ServiceExt::oneshot`, and assert on the response
//! status + body.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use mai_api::audit::MemoryAuditWriter;
use mai_api::auth::AuthState;
use mai_api::config::ServerConfig;
use mai_api::production_guard::{RuntimeChecks, RuntimeOutcome};
use mai_api::routes::build_router;
use mai_api::ship_profile::{ShipProfile, parse_ship_profile};
use mai_api::state::{AppState, ShipReadiness};
use mai_api::trust_builder::TrustExchangeMode;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;
use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_scheduler::DefaultScheduler;

// -- Shared Test Stubs ---------------------------------------------------

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
        _: &str,
        _: &[u8],
    ) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }
    async fn append_audit_entry(&self, _: &[u8]) -> Result<(), mai_core::vault::VaultError> {
        Ok(())
    }
    async fn verify_signature(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<bool, mai_core::vault::VaultError> {
        Ok(true)
    }
}

fn base_app_state() -> AppState {
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
    let auth = AuthState::local_trust();
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

/// Baseline production-mode ship profile that every `PROD-CONFIG/*/*`
/// check accepts. Used by readiness tests to ensure the report
/// reflects the loaded profile rather than an empty default.
fn baseline_ship_profile() -> ShipProfile {
    let toml = r#"
[profile]
name = "ship"
mode = "production"
allow_demo_defaults = false
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"
require_sealed_master_key = true
require_pqc = true
allow_stub = false

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"
require_hash_chain = true
require_pqc_checkpoints = true
require_encryption_at_rest = true
allow_memory_writer = false
allow_null_sealer = false

[trust]
anchors_dir = "/etc/mai/trust-anchors"
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"
allow_accept_all_verifier = false
allow_local_dev_exchange = false
require_trust_anchor = true
require_bundle_on_boot = true

[auth]
auth_keys_path = "/etc/mai/auth_keys.toml"
allow_internal_profile_header = false
require_nonempty_key_store = true

[dashboard]
enabled = true
allow_default_admin_token = false

[network]
bind_address = "127.0.0.1"
tls_mode = "reverse-proxy-required"
require_forwarded_proto_header = false

[observability]
log_format = "json"
log_rotation = true
metrics_exporter = "prometheus"
alerts_enabled = true
"#;
    parse_ship_profile(toml).expect("baseline production profile parses")
}

fn all_pass_runtime() -> RuntimeChecks {
    RuntimeChecks {
        vault_opened: Some(RuntimeOutcome::pass("vault opened (test)")),
        api_audit_wal_ready: Some(RuntimeOutcome::pass("WAL opened (test)")),
        compliance_sealer_real: Some(RuntimeOutcome::pass("AEAD sealer (test)")),
        compliance_signer_real: Some(RuntimeOutcome::pass("audit signer (test)")),
        trust_bundle_verified: Some(RuntimeOutcome::pass("bundle v-test verified")),
        auth_keys_nonempty: Some(RuntimeOutcome::pass("1 key loaded (test)")),
        auth_internal_bypass_consistent: Some(RuntimeOutcome::pass(
            "runtime bypass matches profile field (test)",
        )),
        policy_modules_loaded: Some(RuntimeOutcome::pass("standard template (test)")),
    }
}

fn request(method: &str, uri: &str, body: &str, profile: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-im-profile", profile)
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ─── /v1/system/production-readiness ────────────────────────────────

#[tokio::test]
async fn readiness_without_ship_profile_returns_422() {
    // AppState::new defaults ship_readiness to None. The handler must
    // reject the request with MAI-1002 (422) rather than returning an
    // empty/default report that would mislead operators.
    let state = base_app_state();
    let app = build_router(state);
    let resp = app
        .oneshot(request(
            "GET",
            "/v1/system/production-readiness",
            "",
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let body_text = std::str::from_utf8(&body).unwrap();
    assert!(
        body_text.contains("ship profile"),
        "expected message about missing ship profile, got: {body_text}"
    );
}

#[tokio::test]
async fn readiness_with_ship_profile_returns_full_report() {
    let profile = baseline_ship_profile();
    let runtime = all_pass_runtime();
    let state = base_app_state().with_ship_readiness(ShipReadiness {
        profile: Arc::new(profile),
        runtime_checks: Arc::new(runtime),
    });
    let app = build_router(state);
    let resp = app
        .oneshot(request(
            "GET",
            "/v1/system/production-readiness",
            "",
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    let report: Value = serde_json::from_slice(&body).expect("response is JSON");
    assert_eq!(report["profile"], "ship");
    let checks = report["checks"].as_array().expect("checks is array");
    assert!(
        checks.len() >= 30,
        "expected many checks, got {}",
        checks.len()
    );
    // The runtime-flipped IDs must have status=pass (lowercase per kebab-case serde).
    let mut saw_vault_runtime = false;
    for check in checks {
        if check["id"] == "PROD-VAULT-100" {
            assert_eq!(check["status"], "pass");
            saw_vault_runtime = true;
        }
    }
    assert!(saw_vault_runtime, "PROD-VAULT-100 should appear in report");
}

#[tokio::test]
async fn readiness_rejects_non_admin_caller() {
    let profile = baseline_ship_profile();
    let state = base_app_state().with_ship_readiness(ShipReadiness {
        profile: Arc::new(profile),
        runtime_checks: Arc::new(all_pass_runtime()),
    });
    let app = build_router(state);
    let resp = app
        .oneshot(request(
            "GET",
            "/v1/system/production-readiness",
            "",
            "kid:Child",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── /v1/auth/exchange_token (profile-aware) ────────────────────────

#[tokio::test]
async fn exchange_token_local_dev_synthetic_mints_token() {
    // AppState defaults trust_exchange_mode to LocalDevSynthetic so the
    // legacy bring-up path keeps working without a ship profile.
    let state = base_app_state();
    let app = build_router(state);
    let body = r#"{"subject_id":"alice","tenant_id":"acme","scopes":["read"]}"#;
    let resp = app
        .oneshot(request(
            "POST",
            "/v1/auth/exchange_token",
            body,
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["mode"], "local-dev");
    assert!(
        body["token"].as_str().unwrap().starts_with("local-dev."),
        "expected synthetic local-dev token, got {}",
        body["token"]
    );
    assert_eq!(body["subject_id"], "alice");
    assert_eq!(body["tenant_id"], "acme");
}

#[tokio::test]
async fn exchange_token_openbao_bridge_returns_503_until_client_lands() {
    // Production profiles select OpenBaoBridge. The live HTTP client
    // has not landed yet, so the handler must fail closed with 503
    // rather than silently fall through to the synthetic mint.
    let state = base_app_state().with_trust_exchange_mode(TrustExchangeMode::OpenBaoBridge);
    let app = build_router(state);
    let body = r#"{"subject_id":"alice"}"#;
    let resp = app
        .oneshot(request(
            "POST",
            "/v1/auth/exchange_token",
            body,
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn exchange_token_disabled_returns_410_gone() {
    let state = base_app_state().with_trust_exchange_mode(TrustExchangeMode::Disabled);
    let app = build_router(state);
    let body = r#"{"subject_id":"alice"}"#;
    let resp = app
        .oneshot(request(
            "POST",
            "/v1/auth/exchange_token",
            body,
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "MAI-5003");
}

#[tokio::test]
async fn exchange_token_disabled_still_validates_request_body() {
    // The request-body validation (subject_id is required) must run
    // BEFORE the mode switch — operators sending an obviously malformed
    // request should get a 422 with a useful error, not a profile
    // banner. This documents the handler's contract.
    let state = base_app_state().with_trust_exchange_mode(TrustExchangeMode::Disabled);
    let app = build_router(state);
    let body = r#"{"subject_id":""}"#;
    let resp = app
        .oneshot(request(
            "POST",
            "/v1/auth/exchange_token",
            body,
            "admin-1:Admin",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
