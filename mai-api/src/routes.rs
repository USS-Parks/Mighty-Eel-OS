//! Route definitions for the MAI REST API.
//!
//! Builds the complete axum Router with all endpoint groups, middleware
//! layers (auth, audit, CORS), and state injection. Route groups:
//!
//! - `/v1/chat/*` - Inference (chat completions)
//! - `/v1/completions` - Completion (alias to chat_completions, SDK compat)
//! - `/v1/embeddings` - Embedding generation
//! - `/v1/generate/*` - Structured output, function calling
//! - `/v1/models/*` - Model listing and management
//! - `/v1/health/*` - System health monitoring
//! - `/v1/power/*` - Power state control
//! - `/v1/registry/*` - Model registry management
//! - `/v1/adapters` - Adapter listing
//! - `/v1/audit/*` - Audit trail access
//! - `/v1/profiles/*` - Family profile queries
//! `/v1/trust/*` - Local trust cache surface
//! `/v1/auth/exchange_token` - Local-dev token exchange stub
//! - `/v1/compliance/*` - Compliance policy / audit / reports / feed

use std::convert::Infallible;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::middleware;
use axum::routing::{any, get, post, post_service};
use tower::service_fn;

use crate::auth::auth_middleware;
use crate::handlers;
use crate::state::AppState;
use crate::streaming;

/// Build the complete API router with all routes and middleware.
///
/// The router is structured in groups matching the API surface spec.
/// Auth middleware runs on all routes, validating API keys and extracting
/// the caller's profile. Health routes are exempt from auth (handled
/// in the middleware itself). Individual handlers enforce permission
/// checks for admin-only operations.
pub fn build_router(state: AppState) -> Router {
    // Inference routes (require inference permission)
    let inference_routes = Router::new()
        .route(
            "/v1/chat/completions",
            post(handlers::inference::chat_completions),
        )
        // SDK compat: /v1/completions aliases to chat_completions handler
        .route(
            "/v1/completions",
            post(handlers::inference::chat_completions),
        )
        .route("/v1/embeddings", post(handlers::inference::embeddings))
        .route(
            "/v1/generate/structured",
            post(handlers::inference::structured_generation),
        )
        .route(
            "/v1/generate/function_call",
            post(handlers::inference::function_call),
        );

    // Model routes (list/detail open, load/unload admin-only)
    let state_for_install = state.clone();
    let model_routes = Router::new()
        .route("/v1/models", get(handlers::models::list_models))
        .route(
            "/v1/models/{model_id}",
            get(handlers::models::get_model).delete(handlers::models::remove_model_handler),
        )
        .route(
            "/v1/models/{model_id}/load",
            post(handlers::models::load_model),
        )
        .route(
            "/v1/models/{model_id}/unload",
            post(handlers::models::unload_model),
        )
        .route(
            "/v1/models/{model_id}/benchmark",
            post(handlers::models::benchmark_model).get(handlers::models::get_model_benchmark),
        )
        .route(
            "/v1/models/discover",
            post(handlers::models::discover_packages),
        )
        .route(
            "/v1/models/install",
            post_service(service_fn(move |req: Request<Body>| {
                let state = state_for_install.clone();
                async move {
                    Ok::<_, Infallible>(handlers::models::install_handler_raw(req, state).await)
                }
            })),
        )
        .route(
            "/v1/models/{model_id}/remove",
            post(handlers::models::remove_model_handler),
        );

    // OTA update routes
    let update_routes = Router::new()
        .route("/v1/updates/check", get(handlers::updates::check_updates))
        .route(
            "/v1/updates/download",
            post(handlers::updates::start_update_download),
        )
        .route("/v1/updates/status", get(handlers::updates::update_status));

    // Health routes (open to all, auth exempt)
    let health_routes = Router::new()
        .route("/v1/health", get(handlers::health::aggregate_health))
        .route("/v1/health/adapters", get(handlers::health::adapter_health))
        .route(
            "/v1/health/hardware",
            get(handlers::health::hardware_health),
        )
        .route("/v1/health/system", get(handlers::health::system_health))
        // J-13: disk/RAM/CPU rollup moved off /v1/health/system to free
        // that path for the adapter-rollup endpoint above. The shape
        // is unchanged; the path is the only break.
        .route(
            "/v1/health/resources",
            get(handlers::health::resources_health),
        )
        // SHIP-11: operational health probes (live / ready / production)
        // distinct from the hardware-oriented endpoints above. See
        // handlers/health.rs for the four-state semantics table.
        .route("/v1/health/live", get(handlers::health::live_probe))
        .route("/v1/health/ready", get(handlers::health::ready_probe))
        .route(
            "/v1/health/production",
            get(handlers::health::production_probe),
        );

    // SHIP-11: Prometheus metrics exposition. Auth-exempt (operator
    // scrapers run host-local). Lives outside `health_routes` because
    // the path is `/v1/metrics`, not `/v1/health/*`.
    let metrics_routes =
        Router::new().route("/v1/metrics", get(handlers::metrics::prometheus_metrics));

    // System routes (mixed permissions, enforced per-handler)
    let system_routes = Router::new()
        .route(
            "/v1/system/airgap",
            get(handlers::system::get_airgap_status),
        )
        // SHIP-07 Slice B: live production-readiness report. Admin-only;
        // 422 when the server booted without a ship profile.
        .route(
            "/v1/system/production-readiness",
            get(handlers::system::production_readiness),
        )
        .route("/v1/power", get(handlers::system::get_power_state))
        // SDK compat: /v1/power/state aliases to get_power_state
        .route("/v1/power/state", get(handlers::system::get_power_state))
        .route(
            "/v1/power/transition",
            post(handlers::system::power_transition),
        )
        .route("/v1/registry", get(handlers::system::get_registry))
        .route("/v1/registry/scan", post(handlers::system::registry_scan))
        .route("/v1/adapters", get(handlers::system::list_adapters))
        .route("/v1/audit/log", get(handlers::system::get_audit_log))
        .route("/v1/profiles", get(handlers::system::list_profiles))
        .route(
            "/v1/profiles/{profile_id}",
            get(handlers::system::get_profile),
        );

    // Telemetry / metrics routes
    let telemetry_routes = Router::new()
        .route(
            "/v1/scheduler/metrics",
            get(handlers::telemetry::scheduler_metrics),
        )
        .route(
            "/v1/scheduler/instances/{id}/metrics",
            get(handlers::telemetry::instance_metrics),
        )
        .route(
            "/v1/scheduler/instances/{id}/health",
            get(handlers::telemetry::instance_health),
        )
        .route(
            "/v1/scheduler/anomalies",
            get(handlers::telemetry::scheduler_anomalies),
        );

    // WebSocket streaming route
    let ws_routes = Router::new().route("/v1/ws", any(streaming::ws::ws_upgrade));

    // Trust Manifold routes
    let trust_routes = Router::new()
        .route("/v1/trust/status", get(handlers::trust::get_trust_status))
        .route("/v1/trust/claims", get(handlers::trust::list_claims))
        .route(
            "/v1/trust/bundle_status",
            get(handlers::trust::bundle_status),
        )
        .route(
            "/v1/trust/revocation_status",
            get(handlers::trust::revocation_status),
        )
        .route("/v1/trust/refresh", post(handlers::trust::force_refresh))
        .route(
            "/v1/trust/openbao_health",
            get(handlers::trust::openbao_health),
        )
        .route(
            "/v1/admin/rotate-credentials",
            post(handlers::trust::rotate_credentials),
        )
        .route(
            "/v1/auth/exchange_token",
            post(handlers::trust::exchange_token),
        );

    // Compliance management routes
    let compliance_routes = Router::new()
        .route(
            "/v1/compliance/status",
            get(handlers::compliance::compliance_status),
        )
        .route(
            "/v1/compliance/policies",
            get(handlers::compliance::list_policies),
        )
        .route(
            "/v1/compliance/policies/reload",
            post(handlers::compliance::reload_policy),
        )
        .route(
            "/v1/compliance/policies/template",
            post(handlers::compliance::apply_template),
        )
        .route(
            "/v1/compliance/policies/{module}",
            get(handlers::compliance::get_policy).put(handlers::compliance::update_policy),
        )
        .route(
            "/v1/compliance/modules/{name}/enable",
            post(handlers::compliance::enable_module),
        )
        .route(
            "/v1/compliance/modules/{name}/disable",
            post(handlers::compliance::disable_module),
        )
        .route(
            "/v1/compliance/audit",
            get(handlers::compliance::query_audit),
        )
        .route(
            "/v1/compliance/audit/verify",
            get(handlers::compliance::verify_audit),
        )
        .route(
            "/v1/compliance/audit/integrity",
            get(handlers::compliance::audit_integrity),
        )
        .route(
            "/v1/compliance/audit/{id}",
            get(handlers::compliance::get_audit_entry),
        )
        .route(
            "/v1/compliance/reports",
            get(handlers::compliance::list_reports),
        )
        .route(
            "/v1/compliance/reports/generate",
            post(handlers::compliance::generate_report),
        )
        .route(
            "/v1/compliance/reports/{id}",
            get(handlers::compliance::get_report).delete(handlers::compliance::delete_report),
        )
        .route(
            "/v1/compliance/reports/{id}/download",
            get(handlers::compliance::download_report),
        )
        .route(
            "/v1/compliance/feed",
            get(handlers::compliance::compliance_feed),
        );

    // Merge all route groups and apply middleware. Middleware layers
    // run *outside in*: the last `.layer(...)` call sees the request
    // first. SHIP-11 middleware needs to wrap auth so the metrics
    // counter sees every request (including 401s) and so the
    // correlation ID is set before any handler — including the auth
    // failure path — has a chance to log.
    Router::new()
        .merge(inference_routes)
        .merge(model_routes)
        .merge(update_routes)
        .merge(health_routes)
        .merge(metrics_routes)
        .merge(system_routes)
        .merge(telemetry_routes)
        .merge(ws_routes)
        .merge(trust_routes)
        .merge(compliance_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // SEC-95 (closes SEC-011-MAI): token-bucket rate limit. Sits
        // OUTSIDE auth so an unauthenticated flood cannot exhaust the
        // auth check, but INSIDE metrics so the 429s are counted by
        // `mai_rate_limited_total`. No-op when `AppState.rate_limiter`
        // is None (legacy bring-up / tests).
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::rate_limit_middleware,
        ))
        // SHIP-11: observability layers (innermost-to-outermost order
        // below means OUTERMOST when the request arrives). Metrics
        // middleware needs the correlation span to be active when it
        // emits its own counter logs, so correlation goes last.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::metrics_middleware,
        ))
        .layer(middleware::from_fn(
            crate::middleware::correlation_middleware,
        ))
        .with_state(state)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::{AppState, Router, build_router};

    /// Compile-time check: build_router must accept AppState and return Router.
    /// Runtime route testing is deferred to integration tests.
    #[test]
    fn test_router_builds() {
        let _: fn(AppState) -> Router = build_router;
    }
}
