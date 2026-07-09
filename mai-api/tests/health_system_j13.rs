//! Integration tests for the `/v1/health/system` adapter rollup
//! and the renamed `/v1/health/resources` endpoint.
//!
//! Validates the JSON shape and overall-verdict semantics. The
//! "no adapters registered" case is the only path the integration
//! harness can exercise without spawning real Python subprocesses;
//! the deeper Degraded / Down folds are covered by unit tests in
//! the handler module itself (when added).

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;
use mai_api::audit::MemoryAuditWriter;
use mai_api::auth::AuthState;
use mai_api::config::ServerConfig;
use mai_api::routes::build_router;
use mai_api::state::AppState;

use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_scheduler::DefaultScheduler;

// ─── Test plumbing ─────────────────────────────────────────────────

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
        _id: &str,
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

fn build_test_state() -> AppState {
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
    let hotswap = Arc::new(RwLock::new(HotSwapManager::new(
        legacy_scheduler,
        registry.clone(),
        health.clone(),
    )));
    let audit_writer = Arc::new(MemoryAuditWriter::new());
    let config = Arc::new(RwLock::new(ServerConfig::default()));
    let auth = AuthState::local_trust();
    let adapter_manager = Arc::new(Mutex::new(AdapterManager::new(FrameworkConfig::default())));
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

fn admin_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-im-profile", "admin-1:Admin")
        .body(Body::empty())
        .unwrap()
}

// ─── Tests ─────────────────────────────────────────────────────────

/// With an empty adapter registry the rollup is vacuously `ok`,
/// `adapters` is an empty object, and `ts` looks like RFC3339.
#[tokio::test]
async fn system_health_empty_registry_is_ok() {
    let app = build_router(build_test_state());
    let resp = app.oneshot(admin_get("/v1/health/system")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(
        body["overall"], "ok",
        "vacuous overall must be ok, got: {body}"
    );
    let adapters = body["adapters"]
        .as_object()
        .expect("adapters must be an object");
    assert!(
        adapters.is_empty(),
        "empty registry should yield empty adapters map, got {adapters:?}"
    );

    let ts = body["ts"].as_str().expect("ts must be a string");
    assert!(ts.contains('T'), "ts must look like RFC3339, got {ts}");
    assert!(
        ts.ends_with('Z') || ts.contains('+') || ts.contains('-'),
        "ts must carry a tz designator, got {ts}"
    );
}

/// The old disk/RAM/CPU body moved from /v1/health/system to
/// /v1/health/resources. Confirm the rename works and the original
/// shape is preserved on the new path.
#[tokio::test]
async fn resources_health_returns_legacy_system_shape() {
    let app = build_router(build_test_state());
    let resp = app
        .oneshot(admin_get("/v1/health/resources"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    for field in [
        "disk_utilization_percent",
        "ram_utilization_percent",
        "cpu_utilization_percent",
    ] {
        assert!(
            body.get(field).is_some(),
            "resources body missing `{field}`: {body}"
        );
    }
}
