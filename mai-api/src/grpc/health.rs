//! MaiHealth gRPC service + standard grpc.health.v1 implementation.
//!
//! Two services live here:
//! 1. MaiHealth: MAI-specific health endpoints (adapter, hardware, system, watch)
//! 2. Health (grpc.health.v1): Standard health checking protocol for load balancers
//!    and service meshes.
//!
//! The Watch RPC pushes HealthResponse updates whenever system health changes.
//! Minimum interval is configurable to avoid flooding clients.

use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::debug;

use super::proto;
use crate::state::AppState;

use mai_core::health::{AdapterStatus, AlertLevel, NetworkState, ThermalState};

// ── Helper Conversions ────────────────────────────────────────────

fn adapter_status_str(status: &AdapterStatus) -> String {
    match status {
        AdapterStatus::Healthy => "healthy".to_string(),
        AdapterStatus::Degraded { .. } => "degraded".to_string(),
        AdapterStatus::Unhealthy { .. } => "unhealthy".to_string(),
        AdapterStatus::Unknown => "unknown".to_string(),
    }
}

fn alert_level_str(level: &AlertLevel) -> String {
    level.as_str().to_string()
}

fn network_state_str(state: &NetworkState) -> String {
    match state {
        NetworkState::AirGapCompliant => "compliant".to_string(),
        NetworkState::Connected => "connected".to_string(),
        NetworkState::NonCompliant { .. } => "non_compliant".to_string(),
    }
}

fn thermal_state_str(state: &ThermalState) -> String {
    match state {
        ThermalState::Normal => "normal".to_string(),
        ThermalState::Elevated => "elevated".to_string(),
        ThermalState::Throttled => "throttled".to_string(),
        ThermalState::Critical => "critical".to_string(),
    }
}

fn overall_status_str(level: &AlertLevel) -> String {
    match level {
        AlertLevel::Normal => "healthy".to_string(),
        AlertLevel::Warn | AlertLevel::Degrade => "degraded".to_string(),
        AlertLevel::Critical | AlertLevel::Shutdown => "unhealthy".to_string(),
    }
}

fn vram_utilization(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0) as f32
    }
}

fn disk_utilization(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0) as f32
    }
}

fn ram_utilization(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0) as f32
    }
}

// ═══════════════════════════════════════════════════════════════════
// MaiHealth Service
// ═══════════════════════════════════════════════════════════════════

/// MAI-specific health service with detailed adapter/hardware/system health.
pub struct MaiHealthService {
    state: AppState,
}

impl MaiHealthService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Build a HealthResponse from current system state.
    async fn build_health_response(&self) -> proto::HealthResponse {
        let health = self.state.health.read().await;
        let snapshot = health.get_snapshot();

        let mut adapters: Vec<proto::AdapterHealthSummary> = snapshot
            .adapters
            .iter()
            .map(|(id, a)| proto::AdapterHealthSummary {
                id: id.clone(),
                status: adapter_status_str(&a.status),
                avg_latency_ms: a.avg_latency_ms,
                error_rate: a.error_rate,
            })
            .collect();
        adapters.sort_by(|a, b| a.id.cmp(&b.id));

        let gpus: Vec<proto::GpuHealthSummary> = snapshot
            .hardware
            .gpus
            .iter()
            .map(|g| proto::GpuHealthSummary {
                device_id: g.device_id.clone(),
                temperature_celsius: g.temperature_celsius,
                vram_utilization_percent: vram_utilization(g.vram_used, g.vram_total),
                power_watts: g.power_watts,
                thermal_state: thermal_state_str(&g.thermal_state),
            })
            .collect();

        proto::HealthResponse {
            status: overall_status_str(&snapshot.alert_level),
            alert_level: alert_level_str(&snapshot.alert_level),
            adapters,
            hardware: Some(proto::HardwareHealthSummary {
                gpus,
                air_gap_status: network_state_str(&snapshot.hardware.network_state),
            }),
            system: Some(proto::SystemHealthSummary {
                disk_utilization_percent: disk_utilization(
                    snapshot.system.disk_used_bytes,
                    snapshot.system.disk_total_bytes,
                ),
                ram_utilization_percent: ram_utilization(
                    snapshot.system.ram_used_bytes,
                    snapshot.system.ram_total_bytes,
                ),
                cpu_utilization_percent: snapshot.system.cpu_utilization * 100.0,
            }),
        }
    }
}

#[tonic::async_trait]
impl proto::mai_health_server::MaiHealth for MaiHealthService {
    /// Aggregate health status.
    async fn get_health(
        &self,
        _request: Request<proto::GetHealthRequest>,
    ) -> Result<Response<proto::HealthResponse>, Status> {
        debug!("gRPC GetHealth");
        let response = self.build_health_response().await;
        Ok(Response::new(response))
    }

    /// Per-adapter health.
    async fn get_adapter_health(
        &self,
        _request: Request<proto::GetAdapterHealthRequest>,
    ) -> Result<Response<proto::AdapterHealthResponse>, Status> {
        debug!("gRPC GetAdapterHealth");
        let health = self.state.health.read().await;
        let snapshot = health.get_snapshot();

        let mut adapters: Vec<proto::AdapterHealthSummary> = snapshot
            .adapters
            .iter()
            .map(|(id, a)| proto::AdapterHealthSummary {
                id: id.clone(),
                status: adapter_status_str(&a.status),
                avg_latency_ms: a.avg_latency_ms,
                error_rate: a.error_rate,
            })
            .collect();
        adapters.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Response::new(proto::AdapterHealthResponse { adapters }))
    }

    /// Hardware health (GPUs, air-gap).
    async fn get_hardware_health(
        &self,
        _request: Request<proto::GetHardwareHealthRequest>,
    ) -> Result<Response<proto::HardwareHealthResponse>, Status> {
        debug!("gRPC GetHardwareHealth");
        let health = self.state.health.read().await;
        let snapshot = health.get_snapshot();

        let gpus: Vec<proto::GpuHealthSummary> = snapshot
            .hardware
            .gpus
            .iter()
            .map(|g| proto::GpuHealthSummary {
                device_id: g.device_id.clone(),
                temperature_celsius: g.temperature_celsius,
                vram_utilization_percent: vram_utilization(g.vram_used, g.vram_total),
                power_watts: g.power_watts,
                thermal_state: thermal_state_str(&g.thermal_state),
            })
            .collect();

        Ok(Response::new(proto::HardwareHealthResponse {
            hardware: Some(proto::HardwareHealthSummary {
                gpus,
                air_gap_status: network_state_str(&snapshot.hardware.network_state),
            }),
        }))
    }

    /// System resource health.
    async fn get_system_health(
        &self,
        _request: Request<proto::GetSystemHealthRequest>,
    ) -> Result<Response<proto::SystemHealthResponse>, Status> {
        debug!("gRPC GetSystemHealth");
        let health = self.state.health.read().await;
        let snapshot = health.get_snapshot();

        Ok(Response::new(proto::SystemHealthResponse {
            system: Some(proto::SystemHealthSummary {
                disk_utilization_percent: disk_utilization(
                    snapshot.system.disk_used_bytes,
                    snapshot.system.disk_total_bytes,
                ),
                ram_utilization_percent: ram_utilization(
                    snapshot.system.ram_used_bytes,
                    snapshot.system.ram_total_bytes,
                ),
                cpu_utilization_percent: snapshot.system.cpu_utilization * 100.0,
            }),
        }))
    }

    /// Server-streaming health watch. Pushes updates on state change.
    type WatchStream = ReceiverStream<Result<proto::HealthResponse, Status>>;

    async fn watch(
        &self,
        request: Request<proto::HealthWatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();
        let interval = Duration::from_secs(if req.interval_secs > 0 {
            req.interval_secs as u64
        } else {
            5
        });

        debug!(
            interval_secs = interval.as_secs(),
            "gRPC Health Watch started"
        );

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let state = self.state.clone();

        tokio::spawn(async move {
            let mut last_status = String::new();
            loop {
                tokio::time::sleep(interval).await;

                let health = state.health.read().await;
                let snapshot = health.get_snapshot();
                let current_status = overall_status_str(&snapshot.alert_level);

                if current_status != last_status || last_status.is_empty() {
                    last_status.clone_from(&current_status);

                    let mut adapters: Vec<proto::AdapterHealthSummary> = snapshot
                        .adapters
                        .iter()
                        .map(|(id, a)| proto::AdapterHealthSummary {
                            id: id.clone(),
                            status: adapter_status_str(&a.status),
                            avg_latency_ms: a.avg_latency_ms,
                            error_rate: a.error_rate,
                        })
                        .collect();
                    adapters.sort_by(|a, b| a.id.cmp(&b.id));

                    let gpus: Vec<proto::GpuHealthSummary> = snapshot
                        .hardware
                        .gpus
                        .iter()
                        .map(|g| proto::GpuHealthSummary {
                            device_id: g.device_id.clone(),
                            temperature_celsius: g.temperature_celsius,
                            vram_utilization_percent: vram_utilization(g.vram_used, g.vram_total),
                            power_watts: g.power_watts,
                            thermal_state: thermal_state_str(&g.thermal_state),
                        })
                        .collect();

                    let resp = proto::HealthResponse {
                        status: current_status,
                        alert_level: alert_level_str(&snapshot.alert_level),
                        adapters,
                        hardware: Some(proto::HardwareHealthSummary {
                            gpus,
                            air_gap_status: network_state_str(&snapshot.hardware.network_state),
                        }),
                        system: Some(proto::SystemHealthSummary {
                            disk_utilization_percent: disk_utilization(
                                snapshot.system.disk_used_bytes,
                                snapshot.system.disk_total_bytes,
                            ),
                            ram_utilization_percent: ram_utilization(
                                snapshot.system.ram_used_bytes,
                                snapshot.system.ram_total_bytes,
                            ),
                            cpu_utilization_percent: snapshot.system.cpu_utilization * 100.0,
                        }),
                    };

                    if tx.send(Ok(resp)).await.is_err() {
                        debug!("health watch client disconnected");
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Standard grpc.health.v1 Service
// ═══════════════════════════════════════════════════════════════════

/// Standard gRPC health checking service (grpc.health.v1).
///
/// Returns SERVING when the MAI server is operational.
/// The Watch RPC streams status changes for load balancer integration.
pub struct GrpcHealthService {
    state: AppState,
}

impl GrpcHealthService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::health_server::Health for GrpcHealthService {
    /// Unary health check. Returns SERVING if healthy.
    async fn check(
        &self,
        request: Request<proto::HealthCheckRequest>,
    ) -> Result<Response<proto::HealthCheckResponse>, Status> {
        let req = request.into_inner();
        debug!(service = %req.service, "grpc.health.v1 Check");

        if !req.service.is_empty() {
            let known_services = [
                "mai.v1.MaiInference",
                "mai.v1.MaiModels",
                "mai.v1.MaiHealth",
                "mai.v1.MaiPower",
                "mai.v1.MaiRegistry",
                "mai.v1.MaiAudit",
            ];
            if !known_services.contains(&req.service.as_str()) {
                return Ok(Response::new(proto::HealthCheckResponse {
                    status: proto::health_check_response::ServingStatus::ServiceUnknown as i32,
                }));
            }
        }

        let health = self.state.health.read().await;
        let snapshot = health.get_snapshot();
        let serving = match snapshot.alert_level {
            AlertLevel::Normal | AlertLevel::Warn | AlertLevel::Degrade => {
                proto::health_check_response::ServingStatus::Serving
            }
            AlertLevel::Critical | AlertLevel::Shutdown => {
                proto::health_check_response::ServingStatus::NotServing
            }
        };

        Ok(Response::new(proto::HealthCheckResponse {
            status: serving as i32,
        }))
    }

    /// Streaming health watch for load balancer integration.
    type WatchStream = ReceiverStream<Result<proto::HealthCheckResponse, Status>>;

    async fn watch(
        &self,
        request: Request<proto::HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let service_name = request.into_inner().service;
        debug!(service = %service_name, "grpc.health.v1 Watch");

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let state = self.state.clone();

        tokio::spawn(async move {
            let mut last_serving = -1i32;
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;

                let health = state.health.read().await;
                let snapshot = health.get_snapshot();
                let serving = match snapshot.alert_level {
                    AlertLevel::Normal | AlertLevel::Warn | AlertLevel::Degrade => {
                        proto::health_check_response::ServingStatus::Serving as i32
                    }
                    AlertLevel::Critical | AlertLevel::Shutdown => {
                        proto::health_check_response::ServingStatus::NotServing as i32
                    }
                };

                if serving != last_serving {
                    last_serving = serving;
                    if tx
                        .send(Ok(proto::HealthCheckResponse { status: serving }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_services_constructable() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiHealthService>();
        _assert_send_sync::<GrpcHealthService>();
    }

    #[test]
    fn test_vram_utilization_zero_total() {
        assert_eq!(vram_utilization(100, 0), 0.0);
    }

    #[test]
    fn test_vram_utilization_half() {
        let result = vram_utilization(500, 1000);
        assert!((result - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_status_strings() {
        assert_eq!(adapter_status_str(&AdapterStatus::Healthy), "healthy");
        assert_eq!(
            adapter_status_str(&AdapterStatus::Degraded {
                reason: "slow".to_string()
            }),
            "degraded"
        );
        assert_eq!(
            adapter_status_str(&AdapterStatus::Unhealthy { missed_beats: 3 }),
            "unhealthy"
        );
        assert_eq!(adapter_status_str(&AdapterStatus::Unknown), "unknown");
    }

    #[test]
    fn test_thermal_state_str() {
        assert_eq!(thermal_state_str(&ThermalState::Normal), "normal");
        assert_eq!(thermal_state_str(&ThermalState::Critical), "critical");
    }

    #[test]
    fn test_overall_status_mapping() {
        assert_eq!(overall_status_str(&AlertLevel::Normal), "healthy");
        assert_eq!(overall_status_str(&AlertLevel::Warn), "degraded");
        assert_eq!(overall_status_str(&AlertLevel::Critical), "unhealthy");
    }
}
