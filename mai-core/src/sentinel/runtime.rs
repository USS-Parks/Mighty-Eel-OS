//! Sentinel Runtime — management of the dedicated sentinel adapter instance.
//!
//! The sentinel runtime tracks:
//! - Whether the sentinel adapter instance is loaded and healthy
//! - Resource budget (VRAM usage)
//! - Load metrics (requests handled, token budget remaining)
//! - Health status (last heartbeat, error count)

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::SentinelConfig;

/// Unique identifier for the sentinel instance.
pub type SentinelInstanceId = String;

/// Health status of the sentinel adapter instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentinelHealth {
    /// Adapter process running, ready to serve.
    Healthy,
    /// Process running but degraded (high latency, errors).
    Degraded,
    /// Process not running or unresponsive.
    Unhealthy,
    /// Not yet initialized.
    Unknown,
}

/// Live runtime state of the sentinel adapter instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelRuntimeSnapshot {
    pub instance_id: SentinelInstanceId,
    pub health: SentinelHealth,
    pub uptime_secs: u64,
    pub total_requests_handled: u64,
    pub total_requests_promoted: u64,
    pub last_error: Option<String>,
    pub vram_used_bytes: u64,
    pub vram_budget_bytes: u64,
    pub is_loaded: bool,
}

/// Manages the lifecycle and state of the sentinel adapter instance.
///
/// Thread-safe: uses interior mutability for concurrent access from
/// multiple tokio tasks (estimator, scheduler, health check).
pub struct SentinelRuntime {
    config: SentinelConfig,
    instance_id: SentinelInstanceId,
    health: RwLock<SentinelHealth>,
    is_loaded: AtomicBool,
    started_at: Instant,
    requests_handled: AtomicU64,
    requests_promoted: AtomicU64,
    last_error: RwLock<Option<String>>,
    vram_used: AtomicU64,
    last_heartbeat: RwLock<Instant>,
    health_check_failures: AtomicU64,
}

impl SentinelRuntime {
    /// Create a new sentinel runtime with the given config.
    pub fn new(config: SentinelConfig) -> Self {
        let instance_id = format!("sentinel:{}", config.model);
        info!(instance = %instance_id, "Sentinel runtime created");
        Self {
            config,
            instance_id,
            health: RwLock::new(SentinelHealth::Unknown),
            is_loaded: AtomicBool::new(false),
            started_at: Instant::now(),
            requests_handled: AtomicU64::new(0),
            requests_promoted: AtomicU64::new(0),
            last_error: RwLock::new(None),
            vram_used: AtomicU64::new(0),
            last_heartbeat: RwLock::new(Instant::now()),
            health_check_failures: AtomicU64::new(0),
        }
    }

    /// The instance identifier (used for scheduler registration).
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// The sentinel model name from config.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Sentinel configuration.
    pub fn config(&self) -> &SentinelConfig {
        &self.config
    }

    /// Mark the sentinel adapter as loaded and healthy.
    pub fn mark_loaded(&self) {
        self.is_loaded.store(true, Ordering::Release);
        *self.health.write().unwrap() = SentinelHealth::Healthy;
        *self.last_heartbeat.write().unwrap() = Instant::now();
        info!(instance = %self.instance_id, "Sentinel instance marked loaded");
    }

    /// Mark the sentinel adapter as unloaded (e.g., on shutdown).
    pub fn mark_unloaded(&self) {
        self.is_loaded.store(false, Ordering::Release);
        *self.health.write().unwrap() = SentinelHealth::Unknown;
        info!(instance = %self.instance_id, "Sentinel instance marked unloaded");
    }

    /// Whether the sentinel adapter is currently loaded.
    pub fn is_loaded(&self) -> bool {
        self.is_loaded.load(Ordering::Acquire)
    }

    /// Current health status.
    pub fn health(&self) -> SentinelHealth {
        *self.health.read().unwrap()
    }

    /// Record that a request was handled by the sentinel (no promotion).
    pub fn record_handled(&self) {
        self.requests_handled.fetch_add(1, Ordering::Relaxed);
        self.update_heartbeat();
    }

    /// Record that a request triggered promotion.
    pub fn record_promoted(&self) {
        self.requests_promoted.fetch_add(1, Ordering::Relaxed);
        self.update_heartbeat();
    }

    /// Record an error from the sentinel adapter.
    pub fn record_error(&self, error: String) {
        *self.last_error.write().unwrap() = Some(error.clone());
        self.health_check_failures.fetch_add(1, Ordering::Relaxed);
        warn!(instance = %self.instance_id, error = %error, "Sentinel error recorded");
        // After 5 consecutive failures, mark as degraded
        if self.health_check_failures.load(Ordering::Relaxed) >= 5 {
            *self.health.write().unwrap() = SentinelHealth::Degraded;
        }
        self.update_heartbeat();
    }

    /// Update VRAM usage estimate.
    pub fn set_vram_used(&self, bytes: u64) {
        self.vram_used.store(bytes, Ordering::Relaxed);
    }

    /// Mark the instance as unhealthy (adapter process died).
    pub fn mark_unhealthy(&self) {
        *self.health.write().unwrap() = SentinelHealth::Unhealthy;
        warn!(instance = %self.instance_id, "Sentinel marked unhealthy");
    }

    /// Update health from a heartbeat. Resets failure count.
    pub fn heartbeat(&self) {
        *self.last_heartbeat.write().unwrap() = Instant::now();
        self.health_check_failures.store(0, Ordering::Relaxed);
        if *self.health.read().unwrap() == SentinelHealth::Unhealthy {
            *self.health.write().unwrap() = SentinelHealth::Degraded;
        }
    }

    /// Check if the sentinel has timed out (no heartbeat within threshold).
    pub fn check_timeout(&self, timeout: Duration) -> bool {
        let elapsed = self.last_heartbeat.read().unwrap().elapsed();
        if elapsed > timeout {
            warn!(
                instance = %self.instance_id,
                elapsed_secs = elapsed.as_secs(),
                timeout_secs = timeout.as_secs(),
                "Sentinel heartbeat timeout"
            );
            *self.health.write().unwrap() = SentinelHealth::Unhealthy;
            true
        } else {
            false
        }
    }

    /// Uptime in seconds since creation.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Total requests handled (without promotion).
    pub fn total_handled(&self) -> u64 {
        self.requests_handled.load(Ordering::Relaxed)
    }

    /// Total requests that triggered promotion.
    pub fn total_promoted(&self) -> u64 {
        self.requests_promoted.load(Ordering::Relaxed)
    }

    /// VRAM budget from config.
    pub fn vram_budget(&self) -> u64 {
        self.config.vram_budget_bytes
    }

    /// Current VRAM usage estimate.
    pub fn vram_used(&self) -> u64 {
        self.vram_used.load(Ordering::Relaxed)
    }

    /// Current error string (if any).
    pub fn last_error(&self) -> Option<String> {
        self.last_error.read().unwrap().clone()
    }

    /// Take a snapshot of the current runtime state.
    pub fn snapshot(&self) -> SentinelRuntimeSnapshot {
        SentinelRuntimeSnapshot {
            instance_id: self.instance_id.clone(),
            health: *self.health.read().unwrap(),
            uptime_secs: self.uptime_secs(),
            total_requests_handled: self.total_handled(),
            total_requests_promoted: self.total_promoted(),
            last_error: self.last_error(),
            vram_used_bytes: self.vram_used(),
            vram_budget_bytes: self.vram_budget(),
            is_loaded: self.is_loaded(),
        }
    }

    fn update_heartbeat(&self) {
        *self.last_heartbeat.write().unwrap() = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_runtime() -> SentinelRuntime {
        SentinelRuntime::new(SentinelConfig::default())
    }

    #[test]
    fn test_initial_state() {
        let rt = default_runtime();
        assert!(!rt.is_loaded());
        assert_eq!(rt.health(), SentinelHealth::Unknown);
        assert_eq!(rt.total_handled(), 0);
        assert_eq!(rt.total_promoted(), 0);
    }

    #[test]
    fn test_mark_loaded() {
        let rt = default_runtime();
        rt.mark_loaded();
        assert!(rt.is_loaded());
        assert_eq!(rt.health(), SentinelHealth::Healthy);
    }

    #[test]
    fn test_record_handled() {
        let rt = default_runtime();
        rt.record_handled();
        assert_eq!(rt.total_handled(), 1);
    }

    #[test]
    fn test_record_promoted() {
        let rt = default_runtime();
        rt.record_promoted();
        assert_eq!(rt.total_promoted(), 1);
    }

    #[test]
    fn test_record_error() {
        let rt = default_runtime();
        rt.mark_loaded();
        assert_eq!(rt.health(), SentinelHealth::Healthy);
        rt.record_error("OOM".to_string());
        assert_eq!(rt.last_error(), Some("OOM".to_string()));
        assert_eq!(rt.health(), SentinelHealth::Healthy); // 1 failure, not yet degraded
    }

    #[test]
    fn test_consecutive_errors_degrade() {
        let rt = default_runtime();
        for _ in 0..5 {
            rt.record_error("err".to_string());
        }
        assert_eq!(rt.health(), SentinelHealth::Degraded);
    }

    #[test]
    fn test_heartbeat_resets_failures() {
        let rt = default_runtime();
        rt.mark_loaded();
        // 3 errors, still healthy (< 5 threshold)
        for _ in 0..3 {
            rt.record_error("err".to_string());
        }
        assert_eq!(rt.health(), SentinelHealth::Healthy);
        rt.heartbeat();
        // Still healthy after heartbeat (failures counter reset)
        assert_eq!(rt.health(), SentinelHealth::Healthy);
    }

    #[test]
    fn test_timeout_check() {
        let rt = default_runtime();
        // With a zero timeout, should immediately time out
        assert!(rt.check_timeout(Duration::from_secs(0)));
        assert_eq!(rt.health(), SentinelHealth::Unhealthy);
    }

    #[test]
    fn test_no_timeout_within_threshold() {
        let rt = default_runtime();
        assert!(!rt.check_timeout(Duration::from_secs(3600)));
        assert_ne!(rt.health(), SentinelHealth::Unhealthy);
    }

    #[test]
    fn test_mark_unhealthy() {
        let rt = default_runtime();
        rt.mark_loaded();
        rt.mark_unhealthy();
        assert_eq!(rt.health(), SentinelHealth::Unhealthy);
    }

    #[test]
    fn test_snapshot() {
        let rt = default_runtime();
        rt.mark_loaded();
        rt.record_handled();
        rt.record_promoted();
        let snap = rt.snapshot();
        assert!(snap.is_loaded);
        assert_eq!(snap.total_requests_handled, 1);
        assert_eq!(snap.total_requests_promoted, 1);
        assert_eq!(snap.vram_budget_bytes, 4 * 1_073_741_824);
    }

    #[test]
    fn test_instance_id_format() {
        let rt = default_runtime();
        assert_eq!(rt.instance_id(), "sentinel:phi-4-mini");
    }

    #[test]
    fn test_heartbeat_from_degraded_after_many_errors() {
        let rt = default_runtime();
        rt.mark_loaded();
        for _ in 0..5 {
            rt.record_error("err".to_string());
        }
        assert_eq!(rt.health(), SentinelHealth::Degraded);
        rt.heartbeat();
        // Heartbeat resets failures but doesn't automatically upgrade from Degraded
        assert_eq!(rt.health(), SentinelHealth::Degraded);
    }

    #[test]
    fn test_heartbeat_recovers_from_unhealthy() {
        let rt = default_runtime();
        rt.mark_loaded();
        rt.mark_unhealthy();
        assert_eq!(rt.health(), SentinelHealth::Unhealthy);
        rt.heartbeat();
        assert_eq!(rt.health(), SentinelHealth::Degraded);
    }

    #[test]
    fn test_vram_tracking() {
        let rt = default_runtime();
        rt.set_vram_used(2_000_000_000);
        assert_eq!(rt.vram_used(), 2_000_000_000);
    }

    #[test]
    fn test_mark_unloaded() {
        let rt = default_runtime();
        rt.mark_loaded();
        assert!(rt.is_loaded());
        rt.mark_unloaded();
        assert!(!rt.is_loaded());
        assert_eq!(rt.health(), SentinelHealth::Unknown);
    }
}
