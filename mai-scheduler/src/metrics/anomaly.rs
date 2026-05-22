use serde::{Deserialize, Serialize};

use super::store::RingBuffer;
use crate::types::InstanceId;

/// Configuration for anomaly detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyConfig {
    #[serde(default = "default_latency_spike_multiplier")]
    pub latency_spike_multiplier: f64,
    #[serde(default = "default_vram_trend_window")]
    pub vram_trend_window: usize,
    #[serde(default = "default_throughput_drop_ratio")]
    pub throughput_drop_ratio: f64,
    #[serde(default = "default_queue_buildup_window")]
    pub queue_buildup_window: usize,
}

fn default_latency_spike_multiplier() -> f64 { 3.0 }
fn default_vram_trend_window() -> usize { 10 }
fn default_throughput_drop_ratio() -> f64 { 0.5 }
fn default_queue_buildup_window() -> usize { 5 }

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            latency_spike_multiplier: default_latency_spike_multiplier(),
            vram_trend_window: default_vram_trend_window(),
            throughput_drop_ratio: default_throughput_drop_ratio(),
            queue_buildup_window: default_queue_buildup_window(),
        }
    }
}

/// Types of anomalies that can be detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnomalyKind {
    LatencySpike,
    MemoryLeak,
    ThroughputDrop,
    QueueBuildup,
}

/// An anomaly event emitted when a threshold is crossed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyEvent {
    pub instance_id: InstanceId,
    pub kind: AnomalyKind,
    pub severity: f64,
    pub timestamp: u64,
    pub description: String,
}

/// Detects anomalies for a single instance from streaming data.
pub struct AnomalyDetector {
    latency_samples: RingBuffer<u64>,
    vram_samples: RingBuffer<u64>,
    throughput_samples: RingBuffer<f64>,
    config: AnomalyConfig,
}

impl AnomalyDetector {
    pub fn new(config: AnomalyConfig) -> Self {
        Self {
            latency_samples: RingBuffer::new(100),
            vram_samples: RingBuffer::new(config.vram_trend_window + 5),
            throughput_samples: RingBuffer::new(50),
            config,
        }
    }

    pub fn record(&mut self, latency_ms: u64, vram_used: u64, throughput: f64) {
        self.latency_samples.push(latency_ms);
        self.vram_samples.push(vram_used);
        self.throughput_samples.push(throughput);
    }

    pub fn check_latency_spike(&self) -> Option<f64> {
        let samples: Vec<u64> = self.latency_samples.iter().copied().collect();
        let count = samples.len();
        if count < 3 {
            return None;
        }
        let (last, rest) = samples.split_last()?;
        let mean = rest.iter().sum::<u64>() as f64 / rest.len() as f64;
        if mean < 1.0 {
            return None;
        }
        let ratio = *last as f64 / mean;
        if ratio > self.config.latency_spike_multiplier {
            Some(ratio)
        } else {
            None
        }
    }

    pub fn check_memory_leak(&self) -> Option<f64> {
        let samples: Vec<u64> = self.vram_samples.iter().copied().collect();
        let count = samples.len();
        if count < 4 {
            return None;
        }
        let indices: Vec<f64> = (0..count).map(|i| i as f64).collect();
        let mean_x = indices.iter().sum::<f64>() / count as f64;
        let mean_y = samples.iter().sum::<u64>() as f64 / count as f64;
        let numerator: f64 = indices.iter().zip(samples.iter())
            .map(|(&x, &y)| (x - mean_x) * (y as f64 - mean_y))
            .sum();
        let denominator: f64 = indices.iter().map(|&x| (x - mean_x).powi(2)).sum();
        if denominator.abs() < 1e-12 {
            return None;
        }
        let slope = numerator / denominator;
        let max_vram = samples.iter().max().copied().unwrap_or(1).max(1) as f64;
        let normalized = (slope / max_vram) * count as f64;
        if normalized > 0.1 {
            Some(normalized)
        } else {
            None
        }
    }

    pub fn check_throughput_drop(&self) -> Option<f64> {
        let samples: Vec<f64> = self.throughput_samples.iter().copied().collect();
        let count = samples.len();
        if count < 5 {
            return None;
        }
        let (recent, older) = samples.split_at(count - 3);
        let recent_avg = recent.iter().sum::<f64>() / recent.len() as f64;
        let older_avg = older.iter().sum::<f64>() / older.len() as f64;
        if older_avg < 0.001 {
            return None;
        }
        let ratio = recent_avg / older_avg;
        if ratio < self.config.throughput_drop_ratio {
            Some(1.0 - ratio)
        } else {
            None
        }
    }

    pub fn check_queue_buildup(&self, current_depth: u32, previous_depths: &[u32]) -> Option<f64> {
        let count = previous_depths.len();
        if count < self.config.queue_buildup_window {
            return None;
        }
        let recent: Vec<u32> = previous_depths.iter().rev().take(self.config.queue_buildup_window).copied().collect();
        let increasing = recent.windows(2).all(|w| w[1] >= w[0]);
        if !increasing {
            return None;
        }
        let first = *recent.first().unwrap_or(&1).max(&1);
        let growth = current_depth as f64 / first as f64;
        if growth > 1.5 {
            Some(growth)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detector() -> AnomalyDetector {
        AnomalyDetector::new(AnomalyConfig::default())
    }

    #[test]
    fn test_latency_spike_detected() {
        let mut d = make_detector();
        for _ in 0..10 {
            d.record(50, 4_000_000_000, 100.0);
        }
        // Inject spike
        d.record(500, 4_000_000_000, 100.0);
        assert!(d.check_latency_spike().is_some());
    }

    #[test]
    fn test_no_false_positive_latency() {
        let mut d = make_detector();
        for _ in 0..10 {
            d.record(50, 4_000_000_000, 100.0);
        }
        d.record(55, 4_000_000_000, 100.0);
        assert!(d.check_latency_spike().is_none());
    }

    #[test]
    fn test_queue_buildup_not_enough_data() {
        let d = make_detector();
        let depths = vec![1, 2];
        assert!(d.check_queue_buildup(3, &depths).is_none());
    }

    #[test]
    fn test_queue_not_buildup() {
        let d = make_detector();
        let depths = vec![5, 5, 5, 5, 5, 5];
        assert!(d.check_queue_buildup(5, &depths).is_none());
    }

    #[test]
    fn test_insufficient_latency_data() {
        let mut d = make_detector();
        d.record(50, 4_000_000_000, 100.0);
        d.record(50, 4_000_000_000, 100.0);
        assert!(d.check_latency_spike().is_none());
    }
}
