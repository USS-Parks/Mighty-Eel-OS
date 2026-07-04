//! Gate A acceptance tests.
//!
//! Verifies the four BUILD-EXECUTION-PLAN.md acceptance criteria at
//! the router level with a strict (production-like) `AuthState`:
//!
//! 1. Missing token returns `401 Unauthorized`.
//! 2. Invalid token returns `401 Unauthorized`.
//! 3. Valid token reaches authorized endpoints (not 401, not 403).
//! 4. Rate limit returns `429 Too Many Requests`.
//!
//! Additional checks:
//! - Header profile spoofing is disabled by default (`X-IM-Profile` alone is
//!   not accepted when `allow_internal_profile_header` is false).
//! - Health endpoints remain auth-exempt under strict mode.

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

use mai_api::audit::MemoryAuditWriter;
use mai_api::auth::{ApiKeyStore, AuthState, RateLimiter};
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

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;

// -- Test Vault Stub -------------------------------------------------------

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

// -- Strict State Helper ---------------------------------------------------

const VALID_KEY: &str = "im-gate-a-valid-key";

/// Build an AppState that uses a non-local-trust AuthState with one configured
/// key. Optional `rate_limit` parameters override the default 60 req/min limit
/// so the rate-limit test can run in a single request burst.
fn build_strict_state(rate_limit: Option<(u32, u64)>) -> AppState {
    let scheduler: Arc<dyn mai_scheduler::Scheduler> = Arc::new(DefaultScheduler::new(
        mai_scheduler::SchedulerConfig::default(),
    ));

    let registry = ModelRegistry::new(Box::new(TestVault));
    let registry = Arc::new(RwLock::new(registry));
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
    // allow_internal_profile_header defaults to false in ApiKeyStore::new();
    // this is the production posture that Gate A demands.
    store.add_key_raw(
        VALID_KEY,
        "test-admin".to_string(),
        ProfileRole::Admin,
        Some("Gate A Admin".to_string()),
    );

    let auth = match rate_limit {
        Some((max, window)) => AuthState::with_rate_limit(store, max, window),
        None => AuthState::new(store, RateLimiter::default_per_minute()),
    };

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

fn request_with_headers(
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

// -- Acceptance Criteria ---------------------------------------------------

#[tokio::test]
async fn gate_a_missing_token_returns_401() {
    let app = build_router(build_strict_state(None));
    let req = request_with_headers("GET", "/v1/models", &[], "");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "missing X-IM-Auth-Token must produce 401",
    );
}

#[tokio::test]
async fn gate_a_invalid_token_returns_401() {
    let app = build_router(build_strict_state(None));
    let req = request_with_headers(
        "GET",
        "/v1/models",
        &[("X-IM-Auth-Token", "im-not-a-real-key")],
        "",
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "unknown X-IM-Auth-Token must produce 401",
    );
}

#[tokio::test]
async fn gate_a_valid_token_passes_auth() {
    let app = build_router(build_strict_state(None));
    let req = request_with_headers("GET", "/v1/models", &[("X-IM-Auth-Token", VALID_KEY)], "");
    let resp = app.oneshot(req).await.unwrap();
    // The handler may still 5xx because no models are loaded, but auth must
    // not block. The key invariant is: not a 401/403 from middleware.
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn gate_a_rate_limit_returns_429() {
    // Limit: 2 requests per 60s window for this key.
    let app = build_router(build_strict_state(Some((2, 60))));
    let make_req =
        || request_with_headers("GET", "/v1/models", &[("X-IM-Auth-Token", VALID_KEY)], "");

    let first = app.clone().oneshot(make_req()).await.unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app.clone().oneshot(make_req()).await.unwrap();
    assert_ne!(second.status(), StatusCode::TOO_MANY_REQUESTS);

    let third = app.oneshot(make_req()).await.unwrap();
    assert_eq!(
        third.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "third request beyond the limit must produce 429",
    );
}

// -- Spoofing Defense ------------------------------------------------------

#[tokio::test]
async fn profile_header_alone_is_rejected_in_strict_mode() {
    let app = build_router(build_strict_state(None));
    let req = request_with_headers("GET", "/v1/models", &[("X-IM-Profile", "admin:admin")], "");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "X-IM-Profile alone must not authenticate when allow_internal_profile_header=false",
    );
}

// -- Auth-Exempt Path Still Works -----------------------------------------

#[tokio::test]
async fn health_endpoint_remains_auth_exempt_under_strict_mode() {
    let app = build_router(build_strict_state(None));
    let req = request_with_headers("GET", "/v1/health", &[], "");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/v1/health must be reachable without auth",
    );
}
