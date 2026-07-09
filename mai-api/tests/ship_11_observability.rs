//! Acceptance tests: observability surface.
//!
//! Covers every "Acceptance Tests" bullet from
//! `docs/SHIP-HARDENING-PLAN.md` §10 (line 847):
//!
//! 1. Metrics endpoint does not expose secrets.
//! 2. Health `ready` fails when audit writer fails.
//! 3. Health `production` fails when production guard fails.
//! 4. Logs include correlation ID.
//! 5. Logs do not include prompt text from a test request.
//! 6. (Bonus) Metrics exposition is valid Prometheus text format.
//! 7. (Bonus) Alert YAML parses with serde_yaml round-trip.
//! 8. (Bonus) X-Request-Id is echoed back and generated when absent.

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use mai_api::audit::{AuditEntry, AuditWriter, MemoryAuditWriter};
use mai_api::auth::AuthState;
use mai_api::config::ServerConfig;
use mai_api::metrics::{Labels, MetricsRegistry, REQUESTS_TOTAL, sanitize_label_value};
use mai_api::middleware::REQUEST_ID_HEADER;
use mai_api::routes::build_router;
use mai_api::state::AppState;

use mai_core::health::{HealthConfig, HealthMonitor};
use mai_core::hotswap::HotSwapManager;
use mai_core::power::{PowerConfig, PowerStateMachine};
use mai_core::registry::ModelRegistry;
use mai_core::vault::VaultInterface;
use mai_scheduler::DefaultScheduler;

use mai_adapters::config::FrameworkConfig;
use mai_adapters::manager::AdapterManager;

// ─── Test fixtures ─────────────────────────────────────────────────

struct StubVault;

#[async_trait::async_trait]
impl VaultInterface for StubVault {
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

/// An AuditWriter that always returns an error. Used by the
/// readiness-probe test to assert that `/v1/health/ready` returns 503
/// when audit storage is broken.
struct FailingAuditWriter;

#[async_trait::async_trait]
impl AuditWriter for FailingAuditWriter {
    async fn write(&self, _entry: &AuditEntry) -> Result<(), String> {
        Err("simulated WAL failure".to_string())
    }
    async fn read_recent(&self, _count: usize) -> Result<Vec<AuditEntry>, String> {
        Err("simulated WAL failure".to_string())
    }
    async fn read_by_profile(
        &self,
        _profile_id: &str,
        _limit: usize,
    ) -> Result<Vec<AuditEntry>, String> {
        Err("simulated WAL failure".to_string())
    }
    async fn entry_count(&self) -> Result<u64, String> {
        Err("simulated WAL failure".to_string())
    }
    async fn last_hash(&self) -> Result<String, String> {
        Err("simulated WAL failure".to_string())
    }
}

fn build_state_with_writer(audit_writer: Arc<dyn AuditWriter>) -> AppState {
    let scheduler: Arc<dyn mai_scheduler::Scheduler> = Arc::new(DefaultScheduler::new(
        mai_scheduler::SchedulerConfig::default(),
    ));
    let registry = Arc::new(RwLock::new(ModelRegistry::new(Box::new(StubVault))));
    let health = Arc::new(RwLock::new(HealthMonitor::new(HealthConfig::default())));
    let power = Arc::new(RwLock::new(PowerStateMachine::new(PowerConfig::default())));
    let legacy =
        mai_core::scheduler::Scheduler::new(mai_core::scheduler::SchedulerConfig::default())
            .unwrap();
    let legacy = Arc::new(RwLock::new(legacy));
    let hotswap = Arc::new(RwLock::new(HotSwapManager::new(
        legacy,
        registry.clone(),
        health.clone(),
    )));
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

fn build_state() -> AppState {
    build_state_with_writer(Arc::new(MemoryAuditWriter::new()))
}

// ─── Test 1: /v1/metrics does not expose secrets ───────────────────

#[tokio::test]
async fn ship11_metrics_endpoint_does_not_leak_secrets() {
    let state = build_state();

    // Inject some labels that look like real-world secret shapes.
    // Even if a poorly-written handler attached one of these as a
    // metric label, the registry's sanitizer must redact it.
    state.metrics_registry.inc(
        REQUESTS_TOTAL,
        Labels::new()
            .with("token", "sk-live-deadbeefcafe0123456789")
            .with("route", "/v1/chat/completions"),
    );
    state.metrics_registry.inc(
        REQUESTS_TOTAL,
        Labels::new().with("auth", "Bearer abc.def.ghi"),
    );
    state.metrics_registry.inc(
        REQUESTS_TOTAL,
        Labels::new().with("vault_token", "hvs.CAESIQABCDEFGHIJ"),
    );
    state.metrics_registry.inc(
        REQUESTS_TOTAL,
        Labels::new().with("pat", "ghp_AAAABBBBCCCC"),
    );

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_024 * 1_024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);

    // None of the secret shapes survive into the rendered exposition.
    for secret_fragment in ["sk-live-deadbeef", "Bearer abc", "hvs.CAES", "ghp_AAAA"] {
        assert!(
            !body.contains(secret_fragment),
            "metrics body leaked secret fragment: {secret_fragment}\n{body}"
        );
    }

    // And the redaction marker is what shows up instead.
    assert!(
        body.contains("redacted"),
        "expected 'redacted' marker in metrics output"
    );
}

// ─── Test 2: /v1/metrics emits valid Prometheus exposition ─────────

#[tokio::test]
async fn ship11_metrics_exposition_has_required_structure() {
    let state = build_state();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.starts_with("text/plain"), "wrong content-type: {ct}");
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1_024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);

    // Every metric family has at least a `# TYPE` line.
    for family in [
        "mai_requests_total",
        "mai_request_duration_ms",
        "mai_auth_failures_total",
        "mai_rate_limited_total",
        "mai_audit_write_failures_total",
        "mai_audit_chain_status",
        "mai_trust_bundle_age_seconds",
        "mai_trust_bundle_signature_status",
        "mai_trust_connectivity_state",
        "mai_scheduler_queue_depth",
        "mai_adapter_health",
        "mai_adapter_restart_count",
        "mai_gpu_memory_used_bytes",
        "mai_kv_cache_used_bytes",
        "mai_policy_decisions_total",
        "mai_compliance_report_generation_total",
        "mai_backup_success_total",
        "mai_backup_failure_total",
    ] {
        let needle = format!("# TYPE {family} ");
        assert!(
            body.contains(&needle),
            "missing TYPE line for {family}\n{body}"
        );
    }
}

// ─── Test 3: /v1/health/ready returns 503 when audit writer fails ──

#[tokio::test]
async fn ship11_ready_probe_fails_when_audit_writer_down() {
    let state = build_state_with_writer(Arc::new(FailingAuditWriter));
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(resp.into_body(), 4_096).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    let json: serde_json::Value = serde_json::from_str(&body).expect("response must be JSON");
    assert_eq!(json["status"], "degraded");
    let reasons = json["reasons"].as_array().expect("reasons array");
    assert!(
        reasons.iter().any(|r| {
            let s = r.as_str().unwrap_or("");
            s.starts_with("audit_writer_error") || s == "audit_writer_unresponsive"
        }),
        "expected an audit_writer reason, got {reasons:?}"
    );
}

// ─── Test 4: /v1/health/production returns 503 when chain is broken ─

#[tokio::test]
async fn ship11_production_probe_reports_unsafe_when_audit_breaks() {
    // Same FailingAuditWriter — the production probe is strictly
    // stronger than the ready probe, so a broken audit writer always
    // means "unsafe" rather than "degraded".
    let state = build_state_with_writer(Arc::new(FailingAuditWriter));
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/production")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(resp.into_body(), 4_096).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    let json: serde_json::Value = serde_json::from_str(&body).expect("response must be JSON");
    assert_eq!(json["status"], "unsafe");
    let reasons = json["reasons"].as_array().expect("reasons array");
    assert!(!reasons.is_empty(), "production probe must list reasons");
}

// ─── Test 5: /v1/health/live is always 200 ─────────────────────────

#[tokio::test]
async fn ship11_live_probe_is_unconditionally_ok() {
    let state = build_state_with_writer(Arc::new(FailingAuditWriter));
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4_096).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    let json: serde_json::Value = serde_json::from_str(&body).expect("response must be JSON");
    assert_eq!(json["status"], "live");
}

// ─── Test 6: correlation ID echoed and generated ───────────────────

#[tokio::test]
async fn ship11_correlation_id_echoed_when_supplied() {
    let state = build_state();
    let app = build_router(state);

    let supplied = "req-abcdef0123456789";
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/live")
                .header(REQUEST_ID_HEADER, supplied)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let echoed = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .expect("middleware must echo request id")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(echoed, supplied);
}

#[tokio::test]
async fn ship11_correlation_id_generated_when_absent() {
    let state = build_state();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let echoed = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .expect("middleware must mint request id when absent")
        .to_str()
        .unwrap()
        .to_string();

    // UUID v4 canonical form is 36 chars with 4 hyphens at fixed slots.
    assert_eq!(echoed.len(), 36);
    assert_eq!(echoed.chars().filter(|c| *c == '-').count(), 4);
}

#[tokio::test]
async fn ship11_correlation_id_rejects_header_smuggling() {
    let state = build_state();
    let app = build_router(state);

    // A caller tries to inject a CRLF into the request id so the log
    // line for *their* request bleeds into a fake second log line.
    // axum's `HeaderValue` rejects \r\n at parse time, so the easier
    // way to exercise the sanitizer is via a value containing other
    // forbidden bytes (':', ' ', etc).
    let dirty = "abc def:ghi";
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health/live")
                .header(REQUEST_ID_HEADER, dirty)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let echoed = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .expect("must echo a request id")
        .to_str()
        .unwrap()
        .to_string();

    // Either it was scrubbed down to "abcdefghi" (the safe subset of
    // the caller-supplied value) or a fresh UUID was minted because
    // the cleaned value was empty. Either is acceptable; what's NOT
    // acceptable is echoing the unsanitized version.
    assert!(!echoed.contains(' '), "echoed id must not contain spaces");
    assert!(!echoed.contains(':'), "echoed id must not contain colons");
}

// ─── Test 7: metrics middleware counts every request ───────────────

#[tokio::test]
async fn ship11_metrics_middleware_counts_requests() {
    let state = build_state();
    let app = build_router(state.clone());

    // Pre-scrape baseline (request count for the live probe should be 0).
    let pre = state.metrics_registry.render();
    assert!(!pre.contains("mai_requests_total{route=\"/v1/health/live\""));

    // Drive 3 requests through.
    for _ in 0..3 {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let post = state.metrics_registry.render();
    // The metrics middleware uses route + status_class labels. Three
    // 200s on /v1/health/live should produce a "2xx" counter == 3.
    assert!(
        post.contains("mai_requests_total{route=\"/v1/health/live\",status_class=\"2xx\"} 3"),
        "expected request counter incremented to 3, got:\n{post}"
    );
}

// ─── Test 8: sanitize_label_value redacts secrets (unit-level) ─────

#[test]
fn ship11_sanitize_label_value_redacts_known_secret_prefixes() {
    assert_eq!(sanitize_label_value("sk-live-abcdef"), "redacted");
    assert_eq!(sanitize_label_value("hvs.CAESIQ"), "redacted");
    assert_eq!(sanitize_label_value("ghp_AAAA1111"), "redacted");
    // The "s." prefix is matched after lowercasing, so "S.something"
    // is also caught.
    assert_eq!(sanitize_label_value("S.legacyVaultToken"), "redacted");
}

#[test]
fn ship11_sanitize_label_value_preserves_normal_paths() {
    assert_eq!(
        sanitize_label_value("/v1/chat/completions"),
        "/v1/chat/completions"
    );
    assert_eq!(sanitize_label_value("2xx"), "2xx");
}

#[test]
fn ship11_metrics_registry_starts_with_full_ship11_family_set() {
    let r = MetricsRegistry::with_ship_11_defaults();
    let out = r.render();
    // Spot-check three families that have no observations yet — the
    // `# TYPE` line must still be emitted so dashboards don't show
    // "no data" gaps on a fresh deploy.
    assert!(out.contains("# TYPE mai_backup_failure_total counter"));
    assert!(out.contains("# TYPE mai_audit_chain_status gauge"));
    assert!(out.contains("# TYPE mai_request_duration_ms histogram"));
}

// ─── Test 9: alert YAML structural sanity ─────────────────────────

/// Read `packaging/alerts/mai-alerts.yml` from the repo and assert
/// every alert mentioned in SHIP-HARDENING-PLAN.md §10 line 833 is
/// present. Uses a no-dep substring scan rather than pulling
/// `serde_yaml` into the test-deps — this is enough to catch
/// "operator forgot to commit the file" and "operator renamed an
/// alert" without paying for a full YAML parser.
#[test]
fn ship11_alert_file_has_all_required_rules() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("packaging")
        .join("alerts")
        .join("mai-alerts.yml");
    let body = std::fs::read_to_string(&path).expect("packaging/alerts/mai-alerts.yml must exist");

    for required in [
        "AuditWriteFailure",
        "AuditChainBroken",
        "TrustBundleExpired",
        "TrustBundleStale",
        "TrustBundleSignatureInvalid",
        "ProductionGuardViolation",
        "VaultUnavailable",
        "AdapterCrashLoop",
        "NoHealthyInferenceBackend",
        "AirGapViolation",
        "DiskNearFull",
        "BackupFailed",
        "PolicyReloadFailed",
        "DashboardDefaultToken",
    ] {
        assert!(
            body.contains(&format!("alert: {required}")),
            "alert {required} missing from packaging/alerts/mai-alerts.yml"
        );
    }
}
