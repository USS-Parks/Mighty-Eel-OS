//! Prometheus-compatible `/v1/metrics` endpoint.
//!
//! Returns the live snapshot of [`crate::metrics::MetricsRegistry`]
//! rendered in the Prometheus text exposition format. The endpoint is
//! intentionally auth-exempt — operators scrape from a host-local
//! Prometheus / VictoriaMetrics scraper that runs on the same machine
//! as `mai-api`, and a `127.0.0.1`-only listener (see
//! `docs/operations/OBSERVABILITY.md`) is the operator-deployed posture. The
//! redaction guarantee on
//! [`crate::metrics::sanitize_label_value`] means the body never
//! contains prompts, completions, API keys, or vault tokens even if a
//! caller mis-instruments a counter.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::IntoResponse;

use crate::state::AppState;

/// `GET /v1/metrics`
///
/// Renders the metrics registry as
/// `text/plain; version=0.0.4; charset=utf-8` — the Prometheus text
/// exposition content-type. The body is deterministic given the
/// current counter / gauge / histogram state; tests in
/// `tests/ship_11_observability.rs` rely on this for assertions.
pub async fn prometheus_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.metrics_registry.render();
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    (StatusCode::OK, headers, body)
}
