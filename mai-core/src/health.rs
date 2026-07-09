//! Health Monitor - Adapter heartbeat, hardware telemetry, and alert escalation
//!
//! Monitors adapter processes via heartbeat, collects hardware telemetry through
//! HIL traits, tracks system resources, and computes alert levels. All telemetry
//! is local-only with 5-minute aggregation windows and 24-hour retention.
//! Telemetry is NEVER transmitted off-device.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::types::AdapterId;

/// Snapshot of complete system health at a point in time
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    /// Per-adapter health status
    pub adapters: HashMap<AdapterId, AdapterHealth>,
    /// Hardware health (GPUs, thermal, memory)
    pub hardware: HardwareHealth,
    /// System resource health (disk, RAM, CPU)
    pub system: SystemHealth,
    /// Computed alert level
    pub alert_level: AlertLevel,
    /// When this snapshot was taken
    pub timestamp: Instant,
}

/// Per-adapter health information
#[derive(Debug, Clone)]
pub struct AdapterHealth {
    /// Adapter identifier
    pub adapter_id: AdapterId,
    /// Current status
    pub status: AdapterStatus,
    /// Time of last successful heartbeat
    pub last_heartbeat: Option<Instant>,
    /// Number of consecutive missed heartbeats
    pub missed_heartbeats: u32,
    /// Requests served since last health check
    pub requests_served: u64,
    /// Average latency over measurement window (ms)
    pub avg_latency_ms: f64,
    /// Error rate over measurement window (0.0 - 1.0)
    pub error_rate: f32,
}

/// Adapter status assessment
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterStatus {
    /// Normal operation
    Healthy,
    /// Elevated error rate or latency, but still serving
    Degraded { reason: String },
    /// Not responding to heartbeats
    Unhealthy { missed_beats: u32 },
    /// No heartbeat data yet (just registered)
    Unknown,
}

/// Hardware health from HIL telemetry
#[derive(Debug, Clone)]
pub struct HardwareHealth {
    /// Per-GPU health
    pub gpus: Vec<GpuHealth>,
    /// Air-gap compliance status
    pub network_state: NetworkState,
}

/// Per-GPU health information
#[derive(Debug, Clone)]
pub struct GpuHealth {
    /// GPU identifier
    pub device_id: String,
    /// Current temperature in Celsius
    pub temperature_celsius: f32,
    /// VRAM total bytes
    pub vram_total: u64,
    /// VRAM used bytes
    pub vram_used: u64,
    /// Current power draw in watts
    pub power_watts: u32,
    /// Thermal state assessment
    pub thermal_state: ThermalState,
}

/// Thermal state of a GPU
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThermalState {
    /// Below warning threshold
    Normal,
    /// Above warning, below throttle
    Elevated,
    /// At or above throttle threshold
    Throttled,
    /// Emergency threshold exceeded
    Critical,
}

/// System resource health
#[derive(Debug, Clone)]
pub struct SystemHealth {
    /// Total disk space in bytes
    pub disk_total_bytes: u64,
    /// Used disk space in bytes
    pub disk_used_bytes: u64,
    /// Total RAM in bytes
    pub ram_total_bytes: u64,
    /// Used RAM in bytes
    pub ram_used_bytes: u64,
    /// CPU utilization (0.0 - 1.0)
    pub cpu_utilization: f32,
}

/// Network state for air-gap verification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkState {
    /// All interfaces down, air-gap switch engaged
    AirGapCompliant,
    /// Network connected (acceptable when air-gap switch disengaged)
    Connected,
    /// Air-gap switch engaged but interface(s) up (VIOLATION)
    NonCompliant { interfaces_up: Vec<String> },
}

/// Alert escalation levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AlertLevel {
    /// Everything nominal
    Normal = 0,
    /// Minor issue detected, monitoring
    Warn = 1,
    /// Service degraded, some requests may fail
    Degrade = 2,
    /// Service at risk, immediate attention needed
    Critical = 3,
    /// System must shut down to prevent damage
    Shutdown = 4,
}

impl AlertLevel {
    /// Display name for logging
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Warn => "Warn",
            Self::Degrade => "Degrade",
            Self::Critical => "Critical",
            Self::Shutdown => "Shutdown",
        }
    }
}

/// Configurable alert rule
#[derive(Debug, Clone)]
pub struct AlertRule {
    /// Metric to watch
    pub metric: HealthMetric,
    /// Threshold value that triggers this rule
    pub threshold: f64,
    /// Alert level to set when threshold is exceeded
    pub level: AlertLevel,
    /// Action to take
    pub action: AlertAction,
}

/// Health metrics that can trigger alerts
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthMetric {
    /// GPU temperature
    GpuTemperature,
    /// VRAM utilization percentage
    VramUtilization,
    /// Adapter error rate
    AdapterErrorRate,
    /// Adapter missed heartbeats
    MissedHeartbeats,
    /// Disk usage percentage
    DiskUsage,
    /// RAM usage percentage
    RamUsage,
    /// CPU utilization percentage
    CpuUtilization,
    /// Air-gap compliance
    AirGapCompliance,
}

/// Action to take when alert fires
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertAction {
    /// Log the alert only
    LogOnly,
    /// Attempt to restart degraded adapter
    RestartAdapter { adapter_id: AdapterId },
    /// Trigger thermal throttle
    ThermalThrottle,
    /// Trigger system shutdown
    EmergencyShutdown,
}

/// Health monitor errors
#[derive(Error, Debug)]
pub enum HealthError {
    /// Adapter not registered
    #[error("Adapter not registered: {0}")]
    AdapterNotRegistered(String),

    /// Air-gap violation detected
    #[error("Air-gap violation: {0}")]
    AirGapViolation(String),

    /// Configuration error
    #[error("Config error: {0}")]
    ConfigError(String),
}

/// Health monitor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Heartbeat interval (how often adapters should send heartbeats)
    pub heartbeat_interval: Duration,
    /// Number of missed heartbeats before declaring unhealthy
    pub max_missed_heartbeats: u32,
    /// Telemetry aggregation window (5 minutes default)
    pub aggregation_window: Duration,
    /// Telemetry retention period (24 hours default)
    pub retention_period: Duration,
    /// GPU thermal warning threshold (Celsius)
    pub thermal_warn_celsius: f32,
    /// GPU thermal throttle threshold (Celsius)
    pub thermal_throttle_celsius: f32,
    /// GPU thermal critical threshold (Celsius)
    pub thermal_critical_celsius: f32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(5),
            max_missed_heartbeats: 3,
            aggregation_window: Duration::from_secs(5 * 60),
            retention_period: Duration::from_secs(24 * 60 * 60),
            thermal_warn_celsius: 75.0,
            thermal_throttle_celsius: 83.0,
            thermal_critical_celsius: 90.0,
        }
    }
}

/// Aggregated telemetry data point (local-only, never transmitted).
///
/// TODO(basho): windows are aggregated but not yet consumed by a reader
/// (dashboard / export); the fields stay until that consumer lands.
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct TelemetryWindow {
    start: Instant,
    end: Instant,
    avg_gpu_temp: f32,
    max_gpu_temp: f32,
    avg_vram_used_pct: f32,
    avg_cpu_pct: f32,
    total_requests: u64,
    total_errors: u64,
}

/// The health monitor. Tracks adapter heartbeats, hardware telemetry, and alerts.
pub struct HealthMonitor {
    config: HealthConfig,
    /// Per-adapter health tracking
    adapter_health: HashMap<AdapterId, AdapterHealth>,
    /// Current hardware health
    hardware_health: HardwareHealth,
    /// Current system health
    system_health: SystemHealth,
    /// Alert rules
    alert_rules: Vec<AlertRule>,
    /// Telemetry history (5-minute windows, 24-hour retention).
    /// TODO(basho): aggregated but not yet read by a consumer.
    #[allow(dead_code)]
    telemetry_history: Vec<TelemetryWindow>,
    /// Current aggregation window start.
    #[allow(dead_code)]
    current_window_start: Instant,
    /// Alert subscribers (callback-style, stored as channel senders in production)
    subscriber_count: usize,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(config: HealthConfig) -> Self {
        Self {
            config,
            adapter_health: HashMap::new(),
            hardware_health: HardwareHealth {
                gpus: Vec::new(),
                network_state: NetworkState::AirGapCompliant,
            },
            system_health: SystemHealth {
                disk_total_bytes: 0,
                disk_used_bytes: 0,
                ram_total_bytes: 0,
                ram_used_bytes: 0,
                cpu_utilization: 0.0,
            },
            alert_rules: Self::default_alert_rules(),
            telemetry_history: Vec::new(),
            current_window_start: Instant::now(),
            subscriber_count: 0,
        }
    }

    /// Register an adapter for health monitoring
    pub fn register_adapter(&mut self, adapter_id: AdapterId) {
        info!(adapter = %adapter_id, "Registering adapter for health monitoring");
        self.adapter_health.insert(
            adapter_id.clone(),
            AdapterHealth {
                adapter_id,
                status: AdapterStatus::Unknown,
                last_heartbeat: None,
                missed_heartbeats: 0,
                requests_served: 0,
                avg_latency_ms: 0.0,
                error_rate: 0.0,
            },
        );
    }

    /// Unregister an adapter
    pub fn unregister_adapter(&mut self, adapter_id: &AdapterId) {
        self.adapter_health.remove(adapter_id);
    }

    /// Record a heartbeat from an adapter
    pub fn record_heartbeat(
        &mut self,
        adapter_id: &AdapterId,
        requests_served: u64,
        avg_latency_ms: f64,
        error_rate: f32,
    ) -> Result<(), HealthError> {
        let health = self
            .adapter_health
            .get_mut(adapter_id)
            .ok_or_else(|| HealthError::AdapterNotRegistered(adapter_id.clone()))?;

        health.last_heartbeat = Some(Instant::now());
        health.missed_heartbeats = 0;
        health.requests_served = requests_served;
        health.avg_latency_ms = avg_latency_ms;
        health.error_rate = error_rate;

        // Assess status based on metrics
        if error_rate > 0.5 {
            let pct = error_rate * 100.0;
            health.status = AdapterStatus::Degraded {
                reason: format!("High error rate: {pct:.1}%"),
            };
        } else if avg_latency_ms > 10_000.0 {
            health.status = AdapterStatus::Degraded {
                reason: format!("High latency: {avg_latency_ms:.0}ms"),
            };
        } else {
            health.status = AdapterStatus::Healthy;
        }

        debug!(
            adapter = %adapter_id,
            status = ?health.status,
            "Heartbeat recorded"
        );

        Ok(())
    }

    /// Check for missed heartbeats across all adapters.
    /// Call this periodically (e.g., every heartbeat_interval).
    pub fn check_heartbeats(&mut self) {
        let now = Instant::now();
        let interval = self.config.heartbeat_interval;
        let max_missed = self.config.max_missed_heartbeats;

        for health in self.adapter_health.values_mut() {
            if let Some(last) = health.last_heartbeat {
                let elapsed = now.duration_since(last);
                if elapsed > interval {
                    // Safety: missed heartbeat count is bounded by practical time/interval ratios
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let missed = (elapsed.as_secs_f64() / interval.as_secs_f64()) as u32;
                    health.missed_heartbeats = missed;

                    if missed >= max_missed {
                        warn!(
                            adapter = %health.adapter_id,
                            missed,
                            "Adapter declared unhealthy"
                        );
                        health.status = AdapterStatus::Unhealthy {
                            missed_beats: missed,
                        };
                    }
                }
            }
            // If no heartbeat ever received and adapter is Unknown, leave it
        }
    }

    /// Update hardware health from HIL telemetry data
    pub fn update_hardware_health(&mut self, gpus: Vec<GpuHealth>, network_state: NetworkState) {
        // Assess thermal state for each GPU
        let gpus_with_state: Vec<GpuHealth> = gpus
            .into_iter()
            .map(|mut gpu| {
                gpu.thermal_state = self.assess_thermal(gpu.temperature_celsius);
                gpu
            })
            .collect();

        self.hardware_health = HardwareHealth {
            gpus: gpus_with_state,
            network_state,
        };
    }

    /// Update system resource health
    pub fn update_system_health(&mut self, system: SystemHealth) {
        self.system_health = system;
    }

    /// Verify air-gap compliance. Returns error if violation detected.
    pub fn verify_air_gap(&self) -> Result<(), HealthError> {
        match &self.hardware_health.network_state {
            NetworkState::AirGapCompliant | NetworkState::Connected => Ok(()),
            NetworkState::NonCompliant { interfaces_up } => Err(HealthError::AirGapViolation(
                format!("Air-gap switch engaged but interfaces up: {interfaces_up:?}"),
            )),
        }
    }

    /// Evaluate all alert rules and return the highest triggered alert level
    pub fn evaluate_alerts(&self) -> AlertLevel {
        let mut max_level = AlertLevel::Normal;

        for rule in &self.alert_rules {
            let current_value = self.get_metric_value(&rule.metric);
            if let Some(value) = current_value
                && value >= rule.threshold
                && rule.level > max_level
            {
                max_level = rule.level;
            }
        }

        // Air-gap violation is always Critical
        if matches!(
            self.hardware_health.network_state,
            NetworkState::NonCompliant { .. }
        ) && AlertLevel::Critical > max_level
        {
            max_level = AlertLevel::Critical;
        }

        // Any unhealthy adapter is at least Warn
        let unhealthy_count = self
            .adapter_health
            .values()
            .filter(|h| matches!(h.status, AdapterStatus::Unhealthy { .. }))
            .count();
        if unhealthy_count > 0 && max_level < AlertLevel::Warn {
            max_level = AlertLevel::Warn;
        }

        max_level
    }

    /// Build a complete health snapshot
    pub fn get_snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            adapters: self.adapter_health.clone(),
            hardware: self.hardware_health.clone(),
            system: self.system_health.clone(),
            alert_level: self.evaluate_alerts(),
            timestamp: Instant::now(),
        }
    }

    /// Subscribe to health events (returns subscriber count for now; in production
    /// this would return a channel receiver)
    pub fn subscribe(&mut self) -> usize {
        self.subscriber_count += 1;
        self.subscriber_count
    }

    /// Add a custom alert rule
    pub fn add_alert_rule(&mut self, rule: AlertRule) {
        self.alert_rules.push(rule);
    }

    /// Get adapter health by ID
    pub fn get_adapter_health(&self, adapter_id: &AdapterId) -> Option<&AdapterHealth> {
        self.adapter_health.get(adapter_id)
    }

    /// Number of registered adapters
    pub fn adapter_count(&self) -> usize {
        self.adapter_health.len()
    }

    /// Number of healthy adapters
    pub fn healthy_adapter_count(&self) -> usize {
        self.adapter_health
            .values()
            .filter(|h| matches!(h.status, AdapterStatus::Healthy))
            .count()
    }

    // ─── Internal helpers ─────────────────────────────────────────────

    /// Get current value of a health metric
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn get_metric_value(&self, metric: &HealthMetric) -> Option<f64> {
        match metric {
            HealthMetric::GpuTemperature => self
                .hardware_health
                .gpus
                .iter()
                .map(|g| f64::from(g.temperature_celsius))
                .reduce(f64::max),
            HealthMetric::VramUtilization => {
                let total: u64 = self.hardware_health.gpus.iter().map(|g| g.vram_total).sum();
                let used: u64 = self.hardware_health.gpus.iter().map(|g| g.vram_used).sum();
                if total > 0 {
                    // Safety: u64 -> f64 may lose precision for very large values, acceptable for percentages
                    Some((used as f64 / total as f64) * 100.0)
                } else {
                    None
                }
            }
            HealthMetric::AdapterErrorRate => self
                .adapter_health
                .values()
                .map(|h| f64::from(h.error_rate))
                .reduce(f64::max),
            HealthMetric::MissedHeartbeats => self
                .adapter_health
                .values()
                .map(|h| f64::from(h.missed_heartbeats))
                .reduce(f64::max),
            HealthMetric::DiskUsage => {
                if self.system_health.disk_total_bytes > 0 {
                    // Safety: u64 -> f64 may lose precision for very large values, acceptable for percentages
                    Some(
                        (self.system_health.disk_used_bytes as f64
                            / self.system_health.disk_total_bytes as f64)
                            * 100.0,
                    )
                } else {
                    None
                }
            }
            HealthMetric::RamUsage => {
                if self.system_health.ram_total_bytes > 0 {
                    // Safety: u64 -> f64 may lose precision for very large values, acceptable for percentages
                    Some(
                        (self.system_health.ram_used_bytes as f64
                            / self.system_health.ram_total_bytes as f64)
                            * 100.0,
                    )
                } else {
                    None
                }
            }
            HealthMetric::CpuUtilization => {
                Some(f64::from(self.system_health.cpu_utilization) * 100.0)
            }
            HealthMetric::AirGapCompliance => {
                match self.hardware_health.network_state {
                    NetworkState::NonCompliant { .. } => Some(1.0), // violation
                    _ => Some(0.0),
                }
            }
        }
    }

    /// Assess thermal state from temperature
    fn assess_thermal(&self, temp_celsius: f32) -> ThermalState {
        if temp_celsius >= self.config.thermal_critical_celsius {
            ThermalState::Critical
        } else if temp_celsius >= self.config.thermal_throttle_celsius {
            ThermalState::Throttled
        } else if temp_celsius >= self.config.thermal_warn_celsius {
            ThermalState::Elevated
        } else {
            ThermalState::Normal
        }
    }

    /// Default alert rules spec
    fn default_alert_rules() -> Vec<AlertRule> {
        vec![
            AlertRule {
                metric: HealthMetric::GpuTemperature,
                threshold: 83.0,
                level: AlertLevel::Warn,
                action: AlertAction::LogOnly,
            },
            AlertRule {
                metric: HealthMetric::GpuTemperature,
                threshold: 90.0,
                level: AlertLevel::Critical,
                action: AlertAction::ThermalThrottle,
            },
            AlertRule {
                metric: HealthMetric::AdapterErrorRate,
                threshold: 0.5,
                level: AlertLevel::Degrade,
                action: AlertAction::LogOnly,
            },
            AlertRule {
                metric: HealthMetric::MissedHeartbeats,
                threshold: 3.0,
                level: AlertLevel::Warn,
                action: AlertAction::LogOnly,
            },
            AlertRule {
                metric: HealthMetric::DiskUsage,
                threshold: 90.0,
                level: AlertLevel::Warn,
                action: AlertAction::LogOnly,
            },
            AlertRule {
                metric: HealthMetric::DiskUsage,
                threshold: 98.0,
                level: AlertLevel::Critical,
                action: AlertAction::LogOnly,
            },
            AlertRule {
                metric: HealthMetric::VramUtilization,
                threshold: 95.0,
                level: AlertLevel::Warn,
                action: AlertAction::LogOnly,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_monitor() -> HealthMonitor {
        HealthMonitor::new(HealthConfig::default())
    }

    #[test]
    fn test_register_and_count() {
        let mut mon = default_monitor();
        mon.register_adapter("ollama:0".to_string());
        mon.register_adapter("vllm:0".to_string());
        assert_eq!(mon.adapter_count(), 2);
    }

    #[test]
    fn test_heartbeat_healthy() {
        let mut mon = default_monitor();
        mon.register_adapter("ollama:0".to_string());

        mon.record_heartbeat(&"ollama:0".to_string(), 100, 50.0, 0.01)
            .unwrap();

        let health = mon.get_adapter_health(&"ollama:0".to_string()).unwrap();
        assert!(matches!(health.status, AdapterStatus::Healthy));
        assert_eq!(health.missed_heartbeats, 0);
    }

    #[test]
    fn test_heartbeat_degraded_high_errors() {
        let mut mon = default_monitor();
        mon.register_adapter("ollama:0".to_string());

        mon.record_heartbeat(&"ollama:0".to_string(), 100, 50.0, 0.6)
            .unwrap();

        let health = mon.get_adapter_health(&"ollama:0".to_string()).unwrap();
        assert!(matches!(health.status, AdapterStatus::Degraded { .. }));
    }

    #[test]
    fn test_heartbeat_not_registered() {
        let mut mon = default_monitor();
        let result = mon.record_heartbeat(&"unknown:0".to_string(), 0, 0.0, 0.0);
        assert!(matches!(result, Err(HealthError::AdapterNotRegistered(_))));
    }

    #[test]
    fn test_air_gap_compliant() {
        let mon = default_monitor();
        assert!(mon.verify_air_gap().is_ok());
    }

    #[test]
    fn test_air_gap_violation() {
        let mut mon = default_monitor();
        mon.update_hardware_health(
            vec![],
            NetworkState::NonCompliant {
                interfaces_up: vec!["eth0".to_string()],
            },
        );
        let result = mon.verify_air_gap();
        assert!(matches!(result, Err(HealthError::AirGapViolation(_))));
    }

    #[test]
    fn test_evaluate_alerts_normal() {
        let mon = default_monitor();
        assert_eq!(mon.evaluate_alerts(), AlertLevel::Normal);
    }

    #[test]
    fn test_evaluate_alerts_thermal_warn() {
        let mut mon = default_monitor();
        mon.update_hardware_health(
            vec![GpuHealth {
                device_id: "gpu:0".to_string(),
                temperature_celsius: 85.0,
                vram_total: 32_000_000_000,
                vram_used: 10_000_000_000,
                power_watts: 300,
                thermal_state: ThermalState::Normal, // will be reassessed
            }],
            NetworkState::AirGapCompliant,
        );
        let level = mon.evaluate_alerts();
        assert!(level >= AlertLevel::Warn);
    }

    #[test]
    fn test_evaluate_alerts_air_gap_critical() {
        let mut mon = default_monitor();
        mon.update_hardware_health(
            vec![],
            NetworkState::NonCompliant {
                interfaces_up: vec!["eth0".to_string()],
            },
        );
        assert_eq!(mon.evaluate_alerts(), AlertLevel::Critical);
    }

    #[test]
    fn test_snapshot() {
        let mut mon = default_monitor();
        mon.register_adapter("ollama:0".to_string());
        mon.record_heartbeat(&"ollama:0".to_string(), 50, 30.0, 0.0)
            .unwrap();

        let snap = mon.get_snapshot();
        assert_eq!(snap.alert_level, AlertLevel::Normal);
        assert_eq!(snap.adapters.len(), 1);
    }

    #[test]
    fn test_thermal_assessment() {
        let mon = default_monitor();
        assert_eq!(mon.assess_thermal(50.0), ThermalState::Normal);
        assert_eq!(mon.assess_thermal(78.0), ThermalState::Elevated);
        assert_eq!(mon.assess_thermal(85.0), ThermalState::Throttled);
        assert_eq!(mon.assess_thermal(92.0), ThermalState::Critical);
    }

    #[test]
    fn test_unhealthy_adapter_raises_alert() {
        let mut mon = default_monitor();
        mon.register_adapter("bad:0".to_string());

        // Manually set to unhealthy
        if let Some(h) = mon.adapter_health.get_mut("bad:0") {
            h.status = AdapterStatus::Unhealthy { missed_beats: 5 };
        }

        let level = mon.evaluate_alerts();
        assert!(level >= AlertLevel::Warn);
    }

    #[test]
    fn test_unregister_adapter() {
        let mut mon = default_monitor();
        mon.register_adapter("ollama:0".to_string());
        assert_eq!(mon.adapter_count(), 1);
        mon.unregister_adapter(&"ollama:0".to_string());
        assert_eq!(mon.adapter_count(), 0);
    }

    #[test]
    fn test_subscribe() {
        let mut mon = default_monitor();
        let count = mon.subscribe();
        assert_eq!(count, 1);
        let count = mon.subscribe();
        assert_eq!(count, 2);
    }
}
