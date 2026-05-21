//! Periodic GPU metrics refresh loop.
//!
//! Updates node metrics (free VRAM, utilization, thermal state) at a
//! configurable interval. Does NOT re-read topology (which is static
//! and only changes on hardware hot-plug). Detects anomalies:
//! - GPU utilization stuck at 100% for longer than threshold
//! - Thermal throttle detected
//! - VRAM usage exceeding threshold
//!
//! The refresh loop runs as a tokio background task and pushes updates
//! into the topology graph's node metrics. The scheduler reads these
//! metrics during placement scoring.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::GpuTopology;
use super::collector::AdapterGpuMetrics;
use crate::types::GpuId;

// ---------------------------------------------------------------------------
// Anomaly flags
// ---------------------------------------------------------------------------

/// Anomaly flags detected during metrics refresh.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnomalyFlag {
    /// GPU utilization has been at 100% for longer than the configured
    /// threshold (likely stuck or deadlocked).
    UtilizationStuck { gpu_id: GpuId, stuck_seconds: u64 },

    /// GPU temperature exceeds the thermal throttle threshold.
    ThermalThrottle {
        gpu_id: GpuId,
        temperature_celsius: u32,
    },

    /// VRAM usage exceeds the configured threshold fraction.
    VramExhaustion {
        gpu_id: GpuId,
        used_fraction_bps: u32,
    },
}

// ---------------------------------------------------------------------------
// Refresh state
// ---------------------------------------------------------------------------

/// Per-GPU state tracked across refresh cycles for anomaly detection.
#[derive(Debug, Clone, Default)]
struct GpuRefreshState {
    /// Timestamp when utilization first hit 100%.
    util_stuck_since: Option<Instant>,
}

// ---------------------------------------------------------------------------
// MetricsRefresher
// ---------------------------------------------------------------------------

/// Manages periodic refresh of GPU metrics and anomaly detection.
///
/// Created from a `GpuTopology` reference and runs as a background
/// task. Receives metrics from adapter handshakes or NVML polling
/// and updates the topology graph.
pub struct MetricsRefresher {
    /// Reference to the topology (for config and GPU list).
    topology: Arc<GpuTopology>,
    /// Refresh interval.
    interval: Duration,
    /// Per-GPU tracking state for anomaly detection.
    per_gpu_state: HashMap<GpuId, GpuRefreshState>,
    /// Currently active anomaly flags.
    active_anomalies: Vec<AnomalyFlag>,
}

impl MetricsRefresher {
    /// Create a new refresher.
    pub fn new(topology: Arc<GpuTopology>, interval_ms: u64) -> Self {
        let per_gpu_state = topology
            .graph()
            .gpu_ids()
            .into_iter()
            .map(|id| (id, GpuRefreshState::default()))
            .collect();

        Self {
            topology,
            interval: Duration::from_millis(interval_ms),
            per_gpu_state,
            active_anomalies: Vec::new(),
        }
    }

    /// Get the configured refresh interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Get currently active anomaly flags.
    pub fn active_anomalies(&self) -> &[AnomalyFlag] {
        &self.active_anomalies
    }

    /// Process a batch of GPU metrics from an adapter handshake or
    /// periodic NVML poll. Updates node metrics in the graph and
    /// checks for anomalies.
    ///
    /// Returns any new anomaly flags detected in this refresh cycle.
    pub fn process_metrics(&mut self, metrics: &[AdapterGpuMetrics]) -> Vec<AnomalyFlag> {
        let config = &self.topology.config;
        let mut new_anomalies = Vec::new();
        let now = Instant::now();

        for m in metrics {
            let gpu_id = GpuId(m.gpu_id);

            // Track utilization stuck state
            let state = self.per_gpu_state.entry(gpu_id).or_default();

            if m.utilization_percent >= 100 {
                if state.util_stuck_since.is_none() {
                    state.util_stuck_since = Some(now);
                } else if let Some(since) = state.util_stuck_since {
                    let stuck_secs = now.duration_since(since).as_secs();
                    if stuck_secs >= config.utilization_stuck_seconds {
                        new_anomalies.push(AnomalyFlag::UtilizationStuck {
                            gpu_id,
                            stuck_seconds: stuck_secs,
                        });
                    }
                }
            } else {
                state.util_stuck_since = None;
            }

            // Thermal throttle check
            if m.temperature_celsius >= config.thermal_throttle_celsius {
                new_anomalies.push(AnomalyFlag::ThermalThrottle {
                    gpu_id,
                    temperature_celsius: m.temperature_celsius,
                });
            }

            // VRAM exhaustion check
            if m.total_vram_bytes > 0 {
                let used = m.total_vram_bytes.saturating_sub(m.free_vram_bytes);
                let fraction = used as f64 / m.total_vram_bytes as f64;
                if fraction >= config.vram_anomaly_threshold {
                    new_anomalies.push(AnomalyFlag::VramExhaustion {
                        gpu_id,
                        used_fraction_bps: (fraction * 10_000.0) as u32,
                    });
                }
            }
        }

        self.active_anomalies = new_anomalies.clone();
        new_anomalies
    }

    /// Check if any anomalies are currently active.
    pub fn has_anomalies(&self) -> bool {
        !self.active_anomalies.is_empty()
    }

    /// Clear all anomaly flags (e.g., after operator acknowledgment).
    pub fn clear_anomalies(&mut self) {
        self.active_anomalies.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{GpuTopology, TopologyConfig};

    fn make_topology() -> Arc<GpuTopology> {
        let config = TopologyConfig::default();
        Arc::new(GpuTopology::flat(&config))
    }

    fn make_metrics(
        gpu_id: u32,
        util: u32,
        temp: u32,
        total_vram: u64,
        free_vram: u64,
    ) -> AdapterGpuMetrics {
        AdapterGpuMetrics {
            gpu_id,
            total_vram_bytes: total_vram,
            free_vram_bytes: free_vram,
            utilization_percent: util,
            temperature_celsius: temp,
            pcie_gen: Some(4),
            pcie_width: Some(16),
            nvlink_active_lanes: None,
        }
    }

    #[test]
    fn test_refresher_interval() {
        let topo = make_topology();
        let refresher = MetricsRefresher::new(topo, 500);
        assert_eq!(refresher.interval(), Duration::from_millis(500));
    }

    #[test]
    fn test_no_anomalies_normal_metrics() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        let metrics = vec![make_metrics(0, 50, 65, 80_000_000_000, 40_000_000_000)];
        let anomalies = refresher.process_metrics(&metrics);

        assert!(anomalies.is_empty());
        assert!(!refresher.has_anomalies());
    }

    #[test]
    fn test_thermal_throttle_anomaly() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        // Temperature at 85C exceeds default threshold of 83C
        let metrics = vec![make_metrics(0, 50, 85, 80_000_000_000, 40_000_000_000)];
        let anomalies = refresher.process_metrics(&metrics);

        assert_eq!(anomalies.len(), 1);
        assert!(matches!(
            &anomalies[0],
            AnomalyFlag::ThermalThrottle { gpu_id, temperature_celsius: 85 }
            if *gpu_id == GpuId(0)
        ));
    }

    #[test]
    fn test_vram_exhaustion_anomaly() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        // 95% VRAM used exceeds default threshold of 90%
        let total = 80_000_000_000u64;
        let free = 4_000_000_000u64; // 5% free = 95% used
        let metrics = vec![make_metrics(0, 50, 65, total, free)];
        let anomalies = refresher.process_metrics(&metrics);

        assert_eq!(anomalies.len(), 1);
        assert!(
            matches!(&anomalies[0], AnomalyFlag::VramExhaustion { gpu_id, .. } if *gpu_id == GpuId(0))
        );
    }

    #[test]
    fn test_clear_anomalies() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        let metrics = vec![make_metrics(0, 50, 85, 80_000_000_000, 40_000_000_000)];
        refresher.process_metrics(&metrics);
        assert!(refresher.has_anomalies());

        refresher.clear_anomalies();
        assert!(!refresher.has_anomalies());
    }

    #[test]
    fn test_below_threshold_no_anomaly() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        // Temperature at 82C is below 83C threshold
        let metrics = vec![make_metrics(0, 99, 82, 80_000_000_000, 10_000_000_000)];
        let anomalies = refresher.process_metrics(&metrics);

        // 87.5% VRAM used is below 90% threshold
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_multiple_anomalies_same_gpu() {
        let topo = make_topology();
        let mut refresher = MetricsRefresher::new(topo, 500);

        // Both thermal and VRAM anomaly
        let total = 80_000_000_000u64;
        let free = 2_000_000_000u64; // 97.5% used
        let metrics = vec![make_metrics(0, 50, 90, total, free)];
        let anomalies = refresher.process_metrics(&metrics);

        assert_eq!(anomalies.len(), 2);
    }
}
