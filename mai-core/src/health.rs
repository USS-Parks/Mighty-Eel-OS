//! Health Monitor - Adapter, hardware, and system health tracking
//!
//! Aggregates telemetry locally, detects failures, and escalates alerts.
//! NEVER transmits data off-device.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::watch;

use crate::types::{AdapterId, GpuIdentifier};

/// Health snapshot at a point in time
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    /// When this snapshot was taken
    pub timestamp: Instant,
    /// Per-adapter health status
    pub adapters: HashMap<AdapterId, AdapterHealth>,
    /// Hardware health metrics
    pub hardware: HardwareHealth,
    /// System resource health
    pub system: SystemHealth,
    /// Whether air-gap switch state is compliant
    pub air_gap_verified: bool,
}

/// Per-adapter health status
#[derive(Debug, Clone)]
pub struct AdapterHealth {
    /// Current status classification
    pub status: AdapterStatus,
    /// Last successful heartbeat time
    pub last_heartbeat: Instant,
    /// Consecutive missed heartbeats
    pub missed_heartbeats: u8,
    /// Rolling average latency (5-min window)
    pub avg_latency_ms: f64,
    /// Error rate in 5-min window (0.0-1.0)
    pub error_rate_5min: f64,
    /// Current VRAM usage
    pub vram_usage_bytes: u64,
    /// Requests currently in flight
    pub active_requests: usize,
}

/// Adapter status classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterStatus {
    /// Heartbeat OK, latency below threshold
    Healthy,
    /// Elevated latency or error rate
    Degraded,
    /// Missed heartbeats >= threshold
    Unhealthy,
    /// Never reported or initialization pending
    Unknown,
}

/// Hardware health metrics (from HIL)
#[derive(Debug, Clone)]
pub struct HardwareHealth {
    /// Per-GPU health data
    pub gpus: HashMap<GpuIdentifier, GpuHealth>,
    /// Total system power draw
    pub power_draw_watts: f64,
    /// Overall thermal state
    pub thermal_state: ThermalState,
}

/// Per-GPU health metrics
#[derive(Debug, Clone)]
pub struct GpuHealth {
    /// Current temperature
    pub temperature_celsius: f64,
    /// Fan speed percentage
    pub fan_speed_percent: u8,
    /// VRAM currently used
    pub vram_used_bytes: u64,
    /// Total VRAM available
    pub vram_total_bytes: u64,
    /// Power limit in watts
    pub power_limit_watts: u64,
    /// Compute utilization percentage
    pub compute_utilization_percent: u8,
}

/// Thermal state classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalState {
    /// Normal operating temperature
    Normal,
    /// Approaching limits
    Elevated,
    /// Performance reduced to control temperature
    Throttled,
    /// Shutdown imminent
    Critical,
}

/// System resource health
#[derive(Debug, Clone)]
pub struct SystemHealth {
    /// CPU load percentage
    pub cpu_load_percent: f64,
    /// RAM currently used
    pub ram_used_bytes: u64,
    /// Total RAM available
    pub ram_total_bytes: u64,
    /// Free space on vault filesystem
    pub disk_vault_free_bytes: u64,
    /// Network interface state (air-gap verification)
    pub network_state: NetworkState,
}

/// Network state for air-gap compliance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    /// Air-gap switch engaged, all interfaces down
    AirGapCompliant,
    /// Normal operation (air-gap not engaged)
    Connected,
    /// CRITICAL: switch engaged but interfaces up
    NonCompliant,
}

/// Alert escalation levels (ordered by severity)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    /// Normal operation, log only
    Normal,
    /// Threshold exceeded, prepare mitigation
    Warn,
    /// Service degradation, activate fallbacks
    Degrade,
    /// Critical failure, isolate component
    Critical,
    /// System unsafe, initiate graceful shutdown
    Shutdown,
}

/// Alert rule definition
#[derive(Debug, Clone)]
pub struct AlertRule {
    /// Which metric to evaluate
    pub metric: HealthMetric,
    /// Threshold value that triggers this rule
    pub threshold: f64,
    /// Evaluation window
    pub window: Duration,
    /// Alert level to escalate to
    pub escalation: AlertLevel,
    /// Action to take
    pub action: AlertAction,
}

/// Metrics that can trigger alerts
#[derive(Debug, Clone, Copy)]
pub enum HealthMetric {
    /// Adapter error rate (0.0-1.0)
    AdapterErrorRate,
    /// Adapter response latency (ms)
    AdapterLatency,
    /// GPU temperature (Celsius)
    GpuTemperature,
    /// VRAM utilization (0.0-1.0)
    VramUtilization,
    /// Vault disk free space (bytes)
    DiskFreeSpace,
    /// Consecutive missed heartbeats
    MissedHeartbeats,
}

/// Actions taken when alert rule fires
#[derive(Debug, Clone, Copy)]
pub enum AlertAction {
    /// Log the event
    Log,
    /// Notify local dashboard
    NotifyDashboard,
    /// Activate adapter fallback chain
    ActivateFallback,
    /// Isolate failing component
    IsolateComponent,
    /// Initiate graceful system shutdown
    InitiateShutdown,
}

/// Health monitor errors
#[derive(Error, Debug)]
pub enum HealthError {
    /// Referenced adapter not registered
    #[error("Adapter {0} not registered")]
    AdapterNotFound(AdapterId),

    /// HIL query for hardware metrics failed
    #[error("HIL query failed: {0}")]
    HilQueryFailed(String),

    /// Air-gap verification detected non-compliance
    #[error("Air-gap verification failed: {0}")]
    AirGapNonCompliant(String),

    /// Telemetry ring buffer full (shouldn't happen with rotation)
    #[error("Telemetry buffer full")]
    BufferFull,
}

/// Health monitor configuration
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Interval between adapter heartbeat checks
    pub heartbeat_interval: Duration,
    /// Missed heartbeats before marking Unhealthy
    pub missed_heartbeats_threshold: u8,
    /// Telemetry aggregation window
    pub telemetry_window: Duration,
    /// Number of windows to retain (288 = 24h at 5min windows)
    pub telemetry_retention_windows: usize,
    /// Interval between alert rule evaluations
    pub alert_check_interval: Duration,
}

/// Main health monitor struct
pub struct HealthMonitor {
    config: HealthConfig,
    adapter_states: HashMap<AdapterId, AdapterHealth>,
    alert_rules: Vec<AlertRule>,
    current_alert_level: AlertLevel,
    health_tx: watch::Sender<HealthSnapshot>,
}

impl HealthMonitor {
    /// Create new health monitor with configuration.
    /// Returns the monitor and a receiver for health snapshot updates.
    pub fn new(config: HealthConfig) -> (Self, watch::Receiver<HealthSnapshot>) {
        let initial = HealthSnapshot {
            timestamp: Instant::now(),
            adapters: HashMap::new(),
            hardware: HardwareHealth {
                gpus: HashMap::new(),
                power_draw_watts: 0.0,
                thermal_state: ThermalState::Normal,
            },
            system: SystemHealth {
                cpu_load_percent: 0.0,
                ram_used_bytes: 0,
                ram_total_bytes: 0,
                disk_vault_free_bytes: 0,
                network_state: NetworkState::AirGapCompliant,
            },
            air_gap_verified: true,
        };

        let (tx, rx) = watch::channel(initial);

        let monitor = Self {
            config,
            adapter_states: HashMap::new(),
            alert_rules: Vec::new(),
            current_alert_level: AlertLevel::Normal,
            health_tx: tx,
        };

        (monitor, rx)
    }

    /// Register an adapter for health monitoring
    pub fn register_adapter(&mut self, adapter_id: AdapterId) {
        // Implementation in Session 07
        todo!()
    }

    /// Record a heartbeat from an adapter
    pub fn record_heartbeat(
        &mut self,
        adapter_id: AdapterId,
        latency_ms: f64,
        error_occurred: bool,
    ) -> Result<(), HealthError> {
        // Implementation in Session 07
        todo!()
    }

    /// Update hardware metrics from HIL
    pub async fn update_hardware_health(&mut self) -> Result<(), HealthError> {
        // Implementation in Session 07
        todo!()
    }

    /// Verify air-gap compliance
    pub async fn verify_air_gap(&self) -> Result<bool, HealthError> {
        // Implementation in Session 07
        todo!()
    }

    /// Evaluate alert rules and escalate if needed
    pub fn evaluate_alerts(&mut self) -> Option<AlertLevel> {
        // Implementation in Session 07
        todo!()
    }

    /// Get current health snapshot (for API server)
    pub fn get_snapshot(&self) -> HealthSnapshot {
        // Implementation in Session 07
        todo!()
    }

    /// Subscribe to health updates (for API streaming)
    pub fn subscribe(&self) -> watch::Receiver<HealthSnapshot> {
        self.health_tx.subscribe()
    }

    /// Add a custom alert rule
    pub fn add_alert_rule(&mut self, rule: AlertRule) {
        self.alert_rules.push(rule);
    }
}
