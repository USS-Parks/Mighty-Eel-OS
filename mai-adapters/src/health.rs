//! Heartbeat monitor for adapter processes.
//!
//! Sends periodic heartbeat requests to each adapter. If an adapter misses
//! `missed_heartbeat_threshold` consecutive heartbeats, it is declared dead
//! and the AdapterManager initiates crash recovery.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Health state tracked per adapter.
#[derive(Debug, Clone)]
pub struct AdapterHealthState {
    /// Adapter name for logging.
    pub name: String,
    /// Last time a heartbeat response was received.
    pub last_heartbeat: Instant,
    /// Number of consecutive missed heartbeats.
    pub missed_count: u32,
    /// Whether the adapter has been declared dead.
    pub declared_dead: bool,
    /// Heartbeat interval.
    interval: Duration,
    /// Threshold before declaring dead.
    threshold: u32,
}

/// Result of a health check cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheckResult {
    /// Adapter is healthy.
    Healthy,
    /// Heartbeat was missed but within threshold.
    Missed { count: u32 },
    /// Adapter declared dead (exceeded threshold).
    Dead { missed_count: u32 },
}

/// Health report for external consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub adapter_name: String,
    pub is_healthy: bool,
    pub missed_heartbeats: u32,
    pub last_heartbeat_ms_ago: u64,
    pub declared_dead: bool,
}

impl AdapterHealthState {
    /// Create a new health state tracker for an adapter.
    pub fn new(name: String, interval_ms: u64, threshold: u32) -> Self {
        Self {
            name,
            last_heartbeat: Instant::now(),
            missed_count: 0,
            declared_dead: false,
            interval: Duration::from_millis(interval_ms),
            threshold,
        }
    }

    /// Record a successful heartbeat.
    pub fn record_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
        self.missed_count = 0;
        self.declared_dead = false;
        debug!(adapter = %self.name, "Heartbeat received");
    }

    /// Check health based on elapsed time since last heartbeat.
    /// Call this on the heartbeat interval tick.
    pub fn check(&mut self) -> HealthCheckResult {
        let elapsed = self.last_heartbeat.elapsed();

        if elapsed <= self.interval {
            return HealthCheckResult::Healthy;
        }

        // Calculate how many intervals have been missed
        let intervals_elapsed = (elapsed.as_millis() / self.interval.as_millis()) as u32;
        self.missed_count = intervals_elapsed.saturating_sub(1);

        if self.missed_count >= self.threshold {
            if !self.declared_dead {
                warn!(
                    adapter = %self.name,
                    missed = self.missed_count,
                    threshold = self.threshold,
                    "Adapter declared dead"
                );
                self.declared_dead = true;
            }
            HealthCheckResult::Dead {
                missed_count: self.missed_count,
            }
        } else {
            warn!(
                adapter = %self.name,
                missed = self.missed_count,
                threshold = self.threshold,
                "Heartbeat missed"
            );
            HealthCheckResult::Missed {
                count: self.missed_count,
            }
        }
    }

    /// Generate a health report snapshot.
    pub fn report(&self) -> HealthReport {
        HealthReport {
            adapter_name: self.name.clone(),
            is_healthy: self.missed_count == 0 && !self.declared_dead,
            missed_heartbeats: self.missed_count,
            last_heartbeat_ms_ago: self.last_heartbeat.elapsed().as_millis() as u64,
            declared_dead: self.declared_dead,
        }
    }

    /// Reset health state (after successful restart).
    pub fn reset(&mut self) {
        self.last_heartbeat = Instant::now();
        self.missed_count = 0;
        self.declared_dead = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_initially() {
        let mut state = AdapterHealthState::new("test".to_string(), 5000, 3);
        assert_eq!(state.check(), HealthCheckResult::Healthy);
    }

    #[test]
    fn test_heartbeat_resets_missed() {
        let mut state = AdapterHealthState::new("test".to_string(), 50, 3);
        // Force last_heartbeat to be old
        state.last_heartbeat = Instant::now() - Duration::from_millis(200);
        assert!(matches!(state.check(), HealthCheckResult::Missed { .. }));

        state.record_heartbeat();
        assert_eq!(state.check(), HealthCheckResult::Healthy);
        assert_eq!(state.missed_count, 0);
    }

    #[test]
    fn test_dead_after_threshold() {
        let mut state = AdapterHealthState::new("test".to_string(), 10, 3);
        // Force last_heartbeat to be very old (4+ intervals ago)
        state.last_heartbeat = Instant::now() - Duration::from_millis(50);
        let result = state.check();
        assert!(matches!(result, HealthCheckResult::Dead { .. }));
        assert!(state.declared_dead);
    }

    #[test]
    fn test_report_accuracy() {
        let state = AdapterHealthState::new("ollama".to_string(), 5000, 3);
        let report = state.report();
        assert_eq!(report.adapter_name, "ollama");
        assert!(report.is_healthy);
        assert_eq!(report.missed_heartbeats, 0);
        assert!(!report.declared_dead);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut state = AdapterHealthState::new("test".to_string(), 10, 3);
        state.missed_count = 5;
        state.declared_dead = true;
        state.reset();
        assert_eq!(state.missed_count, 0);
        assert!(!state.declared_dead);
    }
}
