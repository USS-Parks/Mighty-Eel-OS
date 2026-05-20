//! Health check handlers for the MAI REST API.
//!
//! Provides aggregate health, per-adapter health, hardware telemetry,
//! and system resource monitoring. All telemetry is local-only and
//! never transmitted off-device.

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use tracing::debug;

use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    AdapterHealthSummary, GpuHealthSummary, HardwareHealthSummary, HealthResponse, ProfileInfo,
    SystemHealthSummary,
};

use mai_core::health::NetworkState;

// ─── Aggregate Health ──────────────────────────────────────────────

/// GET /v1/health
///
/// Returns aggregate system health: adapter statuses, hardware state,
/// system resources, and computed alert level. Available to all profiles
/// (health is not a privileged operation).
pub async fn aggregate_health(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let health = state.health.read().await;
    let snapshot = health.get_snapshot();

    let adapters: Vec<AdapterHealthSummary> = snapshot
        .adapters
        .values()
        .map(AdapterHealthSummary::from)
        .collect();

    let gpus: Vec<GpuHealthSummary> = snapshot
        .hardware
        .gpus
        .iter()
        .map(GpuHealthSummary::from)
        .collect();

    let air_gap_status = match &snapshot.hardware.network_state {
        NetworkState::AirGapCompliant => "compliant",
        NetworkState::Connected => "connected",
        NetworkState::NonCompliant { .. } => "non_compliant",
    };

    let disk_pct = if snapshot.system.disk_total_bytes > 0 {
        (snapshot.system.disk_used_bytes as f32 / snapshot.system.disk_total_bytes as f32) * 100.0
    } else {
        0.0
    };
    let ram_pct = if snapshot.system.ram_total_bytes > 0 {
        (snapshot.system.ram_used_bytes as f32 / snapshot.system.ram_total_bytes as f32) * 100.0
    } else {
        0.0
    };

    let overall_status = match snapshot.alert_level {
        mai_core::health::AlertLevel::Normal => "healthy",
        mai_core::health::AlertLevel::Warn => "healthy",
        mai_core::health::AlertLevel::Degrade => "degraded",
        mai_core::health::AlertLevel::Critical => "unhealthy",
        mai_core::health::AlertLevel::Shutdown => "unhealthy",
    };

    let response = HealthResponse {
        status: overall_status.to_string(),
        alert_level: crate::types::alert_level_to_string(snapshot.alert_level),
        adapters,
        hardware: HardwareHealthSummary {
            gpus,
            air_gap_status: air_gap_status.to_string(),
        },
        system: SystemHealthSummary {
            disk_utilization_percent: disk_pct,
            ram_utilization_percent: ram_pct,
            cpu_utilization_percent: snapshot.system.cpu_utilization * 100.0,
        },
    };

    debug!(
        status = %response.status,
        alert = %response.alert_level,
        adapters = response.adapters.len(),
        "Health snapshot served"
    );

    Ok(Json(response))
}

// ─── Adapter Health ────────────────────────────────────────────────

/// GET /v1/health/adapters
///
/// Returns per-adapter health details. Adapter identifiers are opaque
/// strings; backend implementation names are never exposed.
pub async fn adapter_health(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let health = state.health.read().await;
    let snapshot = health.get_snapshot();

    let adapters: Vec<AdapterHealthSummary> = snapshot
        .adapters
        .values()
        .map(AdapterHealthSummary::from)
        .collect();

    Ok(Json(serde_json::json!({
        "adapters": adapters,
        "total": adapters.len(),
        "healthy": adapters.iter().filter(|a| a.status == "healthy").count(),
    })))
}

// ─── Hardware Health ───────────────────────────────────────────────

/// GET /v1/health/hardware
///
/// Returns GPU temperatures, VRAM utilization, power draw, thermal state,
/// and air-gap compliance. All data sourced from HIL telemetry.
pub async fn hardware_health(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let health = state.health.read().await;
    let snapshot = health.get_snapshot();

    let gpus: Vec<GpuHealthSummary> = snapshot
        .hardware
        .gpus
        .iter()
        .map(GpuHealthSummary::from)
        .collect();

    let air_gap_status = match &snapshot.hardware.network_state {
        NetworkState::AirGapCompliant => "compliant",
        NetworkState::Connected => "connected",
        NetworkState::NonCompliant { .. } => "non_compliant",
    };

    Ok(Json(HardwareHealthSummary {
        gpus,
        air_gap_status: air_gap_status.to_string(),
    }))
}

// ─── System Health ─────────────────────────────────────────────────

/// GET /v1/health/system
///
/// Returns disk, RAM, and CPU utilization percentages. All metrics
/// are computed locally and never transmitted off-device.
pub async fn system_health(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let health = state.health.read().await;
    let snapshot = health.get_snapshot();

    let disk_pct = if snapshot.system.disk_total_bytes > 0 {
        (snapshot.system.disk_used_bytes as f32 / snapshot.system.disk_total_bytes as f32) * 100.0
    } else {
        0.0
    };
    let ram_pct = if snapshot.system.ram_total_bytes > 0 {
        (snapshot.system.ram_used_bytes as f32 / snapshot.system.ram_total_bytes as f32) * 100.0
    } else {
        0.0
    };

    Ok(Json(SystemHealthSummary {
        disk_utilization_percent: disk_pct,
        ram_utilization_percent: ram_pct,
        cpu_utilization_percent: snapshot.system.cpu_utilization * 100.0,
    }))
}
