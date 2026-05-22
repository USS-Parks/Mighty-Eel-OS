//! Telemetry/metrics query handlers for the MAI REST API.
//!
//! Provides access to scheduler metrics, health scores, anomaly events,
//! and per-instance telemetry. Used for monitoring and debugging.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::Serialize;
use tracing::warn;

use crate::errors::ApiError;
use crate::state::AppState;

/// GET /v1/scheduler/metrics
///
/// Returns global scheduler metrics.
pub async fn scheduler_metrics(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let cluster = state.scheduler.cluster_metrics();
    match serde_json::to_value(&cluster) {
        Ok(val) => Ok(Json(val)),
        Err(e) => {
            warn!("Failed to serialize cluster metrics: {e}");
            Err(ApiError::InternalError)
        }
    }
}

/// GET /v1/scheduler/instances/{id}/metrics
///
/// Returns per-instance metrics.
pub async fn instance_metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let collector = &state.metrics_collector;
    let instance_id = mai_scheduler::InstanceId::new(&id);
    let lifecycles = collector.recent_lifecycles();
    let instance_lifecycles: Vec<_> = lifecycles
        .into_iter()
        .filter(|l| l.instance_id == instance_id)
        .collect();

    #[derive(Serialize)]
    struct InstanceMetricsResponse {
        recent_requests: usize,
        prediction_error: Option<f64>,
        lifecycles: Vec<serde_json::Value>,
    }

    let response = InstanceMetricsResponse {
        recent_requests: instance_lifecycles.len(),
        prediction_error: collector.prediction_error(&instance_id),
        lifecycles: instance_lifecycles
            .into_iter()
            .map(|l| serde_json::to_value(&l).unwrap_or_default())
            .collect(),
    };

    Ok(Json(response))
}

/// GET /v1/scheduler/instances/{id}/health
///
/// Returns health score for an instance.
pub async fn instance_health(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let collector = &state.metrics_collector;
    let instance_id = mai_scheduler::InstanceId::new(&id);
    let cluster = state.scheduler.cluster_metrics();

    // Build a minimal InstanceMetrics from cluster data for query
    let metrics = mai_scheduler::InstanceMetrics {
        queue_depth: cluster.total_queue_depth,
        ..mai_scheduler::InstanceMetrics::default()
    };

    match collector.instance_health_score(&instance_id, &metrics) {
        Some(score) => match serde_json::to_value(&score) {
            Ok(val) => Ok(Json(val)),
            Err(e) => {
                warn!("Failed to serialize health score: {e}");
                Err(ApiError::InternalError)
            }
        },
        None => Ok(Json(serde_json::json!({
            "instance_id": id,
            "health_score": null,
            "message": "No data yet for this instance"
        }))),
    }
}

/// GET /v1/scheduler/anomalies
///
/// Returns recent anomaly events.
pub async fn scheduler_anomalies(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let anomalies = state.metrics_collector.recent_anomalies();
    match serde_json::to_value(&anomalies) {
        Ok(val) => Ok(Json(val)),
        Err(e) => {
            warn!("Failed to serialize anomalies: {e}");
            Err(ApiError::InternalError)
        }
    }
}
