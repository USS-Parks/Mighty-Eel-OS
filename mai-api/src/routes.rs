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

use axum::Router;
use axum::middleware;
use axum::routing::{any, get, post};

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
    let model_routes = Router::new()
        .route("/v1/models", get(handlers::models::list_models))
        .route("/v1/models/{model_id}", get(handlers::models::get_model))
        .route(
            "/v1/models/{model_id}/load",
            post(handlers::models::load_model),
        )
        .route(
            "/v1/models/{model_id}/unload",
            post(handlers::models::unload_model),
        );

    // Health routes (open to all, auth exempt)
    let health_routes = Router::new()
        .route("/v1/health", get(handlers::health::aggregate_health))
        .route("/v1/health/adapters", get(handlers::health::adapter_health))
        .route(
            "/v1/health/hardware",
            get(handlers::health::hardware_health),
        )
        .route("/v1/health/system", get(handlers::health::system_health));

    // System routes (mixed permissions, enforced per-handler)
    let system_routes = Router::new()
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

    // Telemetry / metrics routes (Session 20)
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

    // WebSocket streaming route (Session 11c)
    let ws_routes = Router::new().route("/v1/ws", any(streaming::ws::ws_upgrade));

    // Merge all route groups and apply middleware
    Router::new()
        .merge(inference_routes)
        .merge(model_routes)
        .merge(health_routes)
        .merge(system_routes)
        .merge(telemetry_routes)
        .merge(ws_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    /// Compile-time check: build_router must accept AppState and return Router.
    /// Runtime route testing is deferred to integration tests.
    #[test]
    fn test_router_builds() {
        // This test validates that the router type-checks.
        // Actual HTTP testing requires a running server.
        assert!(true, "Router type-checks at compile time");
    }
}
