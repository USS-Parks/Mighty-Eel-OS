use serde::{Deserialize, Serialize};

use super::store::RingBuffer;
use crate::types::InstanceMetrics;

/// Configuration for health scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// How many recent requests to consider for latency variance. Default: 50.
    #[serde(default = "default_window")]
    pub latency_variance_window: usize,
    /// How many recent requests for error rate. Default: 100.
    #[serde(default = "default_window")]
    pub error_rate_window: usize,
    /// How many recent data points for VRAM trend. Default: 20.
    #[serde(default = "default_vram_trend_window")]
    pub vram_trend_window: usize,
    /// Threshold below which an instance is marked unhealthy. Default: 0.3.
    #[serde(default = "default_health_threshold")]
    pub health_threshold: f64,
}

fn default_window() -> usize {
    50
}

fn default_vram_trend_window() -> usize {
    20
}

fn default_health_threshold() -> f64 {
    0.3
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            latency_variance_window: default_window(),
            error_rate_window: default_window(),
            vram_trend_window: default_vram_trend_window(),
            health_threshold: default_health_threshold(),
        }
    }
}

/// A snapshot of an instance's health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthScore {
    pub overall: f64,
    pub latency_consistency: f64,
    pub error_rate: f64,
    pub memory_stability: f64,
    pub throughput_ratio: f64,
}

impl HealthScore {
    pub fn is_healthy(&self, threshold: f64) -> bool {
        self.overall >= threshold
    }
}

/// Tracks health metrics for a single instance.
pub struct InstanceHealthTracker {
    latency_samples: RingBuffer<u64>,
    error_count: u64,
    total_count: u64,
    vram_samples: RingBuffer<u64>,
    throughput_samples: RingBuffer<f64>,
}

impl InstanceHealthTracker {
    pub fn new(config: &HealthConfig) -> Self {
        Self {
            latency_samples: RingBuffer::new(config.latency_variance_window),
            error_count: 0,
            total_count: 0,
            vram_samples: RingBuffer::new(config.vram_trend_window),
            throughput_samples: RingBuffer::new(config.error_rate_window),
        }
    }

    pub fn record_completion(&mut self, actual_latency_ms: u64, is_error: bool, vram_used: u64, tokens_per_sec: f64) {
        self.latency_samples.push(actual_latency_ms);
        self.total_count += 1;
        if is_error {
            self.error_count += 1;
        }
        self.vram_samples.push(vram_used);
        self.throughput_samples.push(tokens_per_sec);
    }

    pub fn score(&self, metrics: &InstanceMetrics, _config: &HealthConfig) -> HealthScore {
        let consistency = self.latency_consistency();
        let error = self.error_rate_score();
        let memory = self.memory_stability();
        let throughput = self.throughput_ratio(metrics);

        let overall = 0.35 * consistency + 0.30 * error + 0.20 * memory + 0.15 * throughput;
        let overall = overall.clamp(0.0, 1.0);

        HealthScore {
            overall,
            latency_consistency: consistency,
            error_rate: error,
            memory_stability: memory,
            throughput_ratio: throughput,
        }
    }

    fn latency_consistency(&self) -> f64 {
        let samples: Vec<u64> = self.latency_samples.iter().copied().collect();
        let count = samples.len();
        if count < 2 {
            return 1.0;
        }
        let mean = samples.iter().sum::<u64>() as f64 / count as f64;
        if mean < 1.0 {
            return 1.0;
        }
        let variance = samples.iter().map(|&v| {
            let diff = v as f64 - mean;
            diff * diff
        }).sum::<f64>() / count as f64;
        let std_dev = variance.sqrt();
        let cv = std_dev / mean; // coefficient of variation
        (1.0 - cv.min(1.0)).max(0.0)
    }

    fn error_rate_score(&self) -> f64 {
        if self.total_count == 0 {
            return 1.0;
        }
        let rate = self.error_count as f64 / self.total_count as f64;
        (1.0 - rate).max(0.0)
    }

    fn memory_stability(&self) -> f64 {
        let samples: Vec<u64> = self.vram_samples.iter().copied().collect();
        let count = samples.len();
        if count < 3 {
            return 1.0;
        }
        // Simple linear regression slope to detect upward trend = leak
        let indices: Vec<f64> = (0..count).map(|i| i as f64).collect();
        let mean_x = indices.iter().sum::<f64>() / count as f64;
        let mean_y = samples.iter().sum::<u64>() as f64 / count as f64;
        let numerator: f64 = indices.iter().zip(samples.iter())
            .map(|(&x, &y)| (x - mean_x) * (y as f64 - mean_y))
            .sum();
        let denominator: f64 = indices.iter().map(|&x| (x - mean_x).powi(2)).sum();

        if denominator.abs() < 1e-12 {
            return 1.0;
        }
        let slope = numerator / denominator;
        // Normalize slope: if vram is growing, penalize
        let max_vram = samples.iter().max().copied().unwrap_or(1).max(1) as f64;
        let normalized_slope = (slope / max_vram).abs();
        (1.0 - normalized_slope.min(1.0)).max(0.0)
    }

    fn throughput_ratio(&self, _metrics: &InstanceMetrics) -> f64 {
        let samples: Vec<f64> = self.throughput_samples.iter().copied().collect();
        let count = samples.len();
        if count < 2 {
            return 1.0;
        }
        let recent: Vec<&f64> = samples.iter().rev().take(5).collect();
        let recent_avg = recent.iter().copied().sum::<f64>() / recent.len() as f64;
        let all_avg = samples.iter().sum::<f64>() / count as f64;
        if all_avg < 0.001 {
            return 1.0;
        }
        let ratio = recent_avg / all_avg;
        ratio.min(1.0).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> HealthConfig {
        HealthConfig::default()
    }

    fn sample_metrics() -> InstanceMetrics {
        InstanceMetrics::default()
    }

    #[test]
    fn test_healthy_instance_scores_high() {
        let mut tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        for i in 0..20 {
            tracker.record_completion(50 + i % 5, false, 4_000_000_000, 100.0);
        }

        let score = tracker.score(&metrics, &default_config());
        assert!(score.overall > 0.8, "healthy score={}", score.overall);
    }

    #[test]
    fn test_high_error_rate_reduces_score() {
        let mut tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        for _ in 0..10 {
            tracker.record_completion(50, true, 4_000_000_000, 100.0);
        }

        let score = tracker.score(&metrics, &default_config());
        // Error rate = 30% weight → overall = 1.0 - 0.3 = 0.7
        assert!(score.overall < 0.75, "degraded score={}", score.overall);
        assert!(score.error_rate < 0.3);
    }

    #[test]
    fn test_latency_spikes_reduce_consistency() {
        let mut tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        for _ in 0..10 {
            tracker.record_completion(50, false, 4_000_000_000, 100.0);
        }
        let stable = tracker.score(&metrics, &default_config());

        // Add high-variance latencies
        for _ in 0..10 {
            tracker.record_completion(500, false, 4_000_000_000, 100.0);
        }
        let spiked = tracker.score(&metrics, &default_config());

        assert!(spiked.latency_consistency < stable.latency_consistency);
    }

    #[test]
    fn test_memory_leak_detection() {
        let mut stable_tracker = InstanceHealthTracker::new(&default_config());
        let mut leak_tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        // Stable: constant VRAM
        for _ in 0..10 {
            stable_tracker.record_completion(50, false, 4_000_000_000, 100.0);
        }
        // Leak: VRAM increasing dramatically over time (10x growth)
        for i in 0..10 {
            leak_tracker.record_completion(50, false, 1_000_000_000 + i as u64 * 10_000_000_000, 100.0);
        }

        let stable_score = stable_tracker.score(&metrics, &default_config());
        let leak_score = leak_tracker.score(&metrics, &default_config());
        assert!(leak_score.memory_stability < stable_score.memory_stability,
            "leak={} should be < stable={}", leak_score.memory_stability, stable_score.memory_stability);
    }

    #[test]
    fn test_health_threshold() {
        let mut tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        for _ in 0..5 {
            tracker.record_completion(50, true, 4_000_000_000, 100.0);
        }
        let score = tracker.score(&metrics, &default_config());
        assert!(!score.is_healthy(0.8));
    }

    #[test]
    fn test_empty_tracker_is_healthy() {
        let tracker = InstanceHealthTracker::new(&default_config());
        let score = tracker.score(&sample_metrics(), &default_config());
        assert!(score.overall > 0.9);
    }

    #[test]
    fn test_throughput_drop_detected() {
        let mut tracker = InstanceHealthTracker::new(&default_config());
        let metrics = sample_metrics();

        // First 10 at high throughput
        for _ in 0..10 {
            tracker.record_completion(50, false, 4_000_000_000, 100.0);
        }
        let high = tracker.score(&metrics, &default_config());

        // Then 5 at very low throughput
        for _ in 0..5 {
            tracker.record_completion(50, false, 4_000_000_000, 10.0);
        }
        let dropped = tracker.score(&metrics, &default_config());

        assert!(dropped.throughput_ratio < high.throughput_ratio);
    }
}
