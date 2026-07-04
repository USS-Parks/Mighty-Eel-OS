//! Per-instance batch metrics tracking.
//!
//! Tracks rolling-window statistics for continuous batching performance:
//! average batch size, utilization, admission rate, eviction-for-admission
//! rate, and queue wait time percentiles.
//!
//! Thread-safe via atomics and mutex-protected percentile tracking.
//! Metrics are consumed by the scheduler scoring engine
//! and exposed through ClusterMetrics.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for batch metrics collection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Rolling window size for average calculations.
    #[serde(default = "default_window_size")]
    pub window_size: usize,

    /// Maximum number of wait-time samples to retain for percentile calculation.
    #[serde(default = "default_max_wait_samples")]
    pub max_wait_samples: usize,
}

fn default_window_size() -> usize {
    100
}

fn default_max_wait_samples() -> usize {
    1000
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            window_size: default_window_size(),
            max_wait_samples: default_max_wait_samples(),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot (read-only view for consumers)
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of batch metrics. Cheap to clone and share.
#[derive(Debug, Clone, Default)]
pub struct BatchMetricsSnapshot {
    /// Average batch size over the rolling window.
    pub avg_batch_size: f64,
    /// Batch utilization: avg_batch_size / max_batch_size (0.0..1.0).
    pub batch_utilization: f64,
    /// Fraction of queued requests that were admitted (0.0..1.0).
    pub admission_rate: f64,
    /// Fraction of admissions that required eviction first.
    pub eviction_for_admission_rate: f64,
    /// Queue wait time: 50th percentile (median).
    pub wait_p50: Duration,
    /// Queue wait time: 95th percentile.
    pub wait_p95: Duration,
    /// Queue wait time: 99th percentile.
    pub wait_p99: Duration,
    /// Total build steps executed.
    pub total_steps: u64,
    /// Total sequences admitted.
    pub total_admitted: u64,
    /// Total sequences completed.
    pub total_completed: u64,
    /// Total admission attempts (admitted + rejected).
    pub total_admission_attempts: u64,
    /// Total admissions that required eviction.
    pub total_eviction_admissions: u64,
}

// ---------------------------------------------------------------------------
// BatchMetrics
// ---------------------------------------------------------------------------

/// Per-instance batch metrics tracker.
///
/// Uses atomics for counters (lock-free increment from multiple tasks)
/// and a Mutex for the rolling window and wait-time samples (write-rare,
/// read on snapshot).
pub struct BatchMetrics {
    // Atomic counters
    total_steps: AtomicU64,
    total_admitted: AtomicU64,
    total_completed: AtomicU64,
    total_admission_attempts: AtomicU64,
    total_eviction_admissions: AtomicU64,

    // Protected state
    inner: Mutex<MetricsInner>,

    // Config
    max_batch_size: u32,
}

struct MetricsInner {
    /// Rolling window of recent batch sizes (for average).
    batch_sizes: VecDeque<u32>,
    /// Rolling window of recent wait times (for percentiles).
    wait_times: VecDeque<Duration>,
    /// Config limits.
    window_size: usize,
    max_wait_samples: usize,
}

impl BatchMetrics {
    /// Create a new metrics tracker for an instance with the given max batch size.
    pub fn new(max_batch_size: u32, config: MetricsConfig) -> Self {
        Self {
            total_steps: AtomicU64::new(0),
            total_admitted: AtomicU64::new(0),
            total_completed: AtomicU64::new(0),
            total_admission_attempts: AtomicU64::new(0),
            total_eviction_admissions: AtomicU64::new(0),
            inner: Mutex::new(MetricsInner {
                batch_sizes: VecDeque::with_capacity(config.window_size),
                wait_times: VecDeque::with_capacity(config.max_wait_samples),
                window_size: config.window_size,
                max_wait_samples: config.max_wait_samples,
            }),
            max_batch_size,
        }
    }

    /// Record the completion of one build step with the given batch size.
    pub fn record_step(&self, batch_size: u32) {
        self.total_steps.fetch_add(1, Ordering::Relaxed);

        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.batch_sizes.len() >= inner.window_size {
            inner.batch_sizes.pop_front();
        }
        inner.batch_sizes.push_back(batch_size);
    }

    /// Record that `count` sequences were admitted to the batch.
    pub fn record_admissions(&self, count: u64) {
        self.total_admitted.fetch_add(count, Ordering::Relaxed);
        self.total_admission_attempts
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Record that `count` admission attempts were rejected (queued).
    pub fn record_rejections(&self, count: u64) {
        self.total_admission_attempts
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Record that `count` admissions required eviction to make room.
    pub fn record_eviction_admissions(&self, count: u64) {
        self.total_eviction_admissions
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Record that `count` sequences completed generation.
    pub fn record_completions(&self, count: u64) {
        self.total_completed.fetch_add(count, Ordering::Relaxed);
    }

    /// Record a queue wait time for a sequence that was admitted.
    pub fn record_wait_time(&self, wait: Duration) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.wait_times.len() >= inner.max_wait_samples {
            inner.wait_times.pop_front();
        }
        inner.wait_times.push_back(wait);
    }

    /// Take a snapshot of current metrics. This is the read path for
    /// the scheduler scoring engine and ClusterMetrics aggregation.
    #[allow(clippy::cast_precision_loss)] // Acceptable: metric values don't need full u64 precision
    pub fn snapshot(&self) -> BatchMetricsSnapshot {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let avg_batch_size = if inner.batch_sizes.is_empty() {
            0.0
        } else {
            let sum: u64 = inner.batch_sizes.iter().map(|&s| u64::from(s)).sum();
            sum as f64 / inner.batch_sizes.len() as f64
        };

        let batch_utilization = if self.max_batch_size > 0 {
            avg_batch_size / f64::from(self.max_batch_size)
        } else {
            0.0
        };

        let total_attempts = self.total_admission_attempts.load(Ordering::Relaxed);
        let total_admitted = self.total_admitted.load(Ordering::Relaxed);
        let total_eviction = self.total_eviction_admissions.load(Ordering::Relaxed);

        let admission_rate = if total_attempts > 0 {
            total_admitted as f64 / total_attempts as f64
        } else {
            1.0 // no attempts = no rejections
        };

        let eviction_for_admission_rate = if total_admitted > 0 {
            total_eviction as f64 / total_admitted as f64
        } else {
            0.0
        };

        let (wait_p50, wait_p95, wait_p99) = compute_percentiles(&inner.wait_times);

        BatchMetricsSnapshot {
            avg_batch_size,
            batch_utilization,
            admission_rate,
            eviction_for_admission_rate,
            wait_p50,
            wait_p95,
            wait_p99,
            total_steps: self.total_steps.load(Ordering::Relaxed),
            total_admitted,
            total_completed: self.total_completed.load(Ordering::Relaxed),
            total_admission_attempts: total_attempts,
            total_eviction_admissions: total_eviction,
        }
    }

    /// The configured max batch size for this instance.
    pub fn max_batch_size(&self) -> u32 {
        self.max_batch_size
    }
}

#[allow(clippy::missing_fields_in_debug)] // Intentionally omitting internal Mutex state
impl std::fmt::Debug for BatchMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchMetrics")
            .field("max_batch_size", &self.max_batch_size)
            .field("total_steps", &self.total_steps.load(Ordering::Relaxed))
            .field(
                "total_admitted",
                &self.total_admitted.load(Ordering::Relaxed),
            )
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Percentile computation
// ---------------------------------------------------------------------------

/// Compute P50, P95, P99 from a deque of durations.
/// Returns (Duration::ZERO, Duration::ZERO, Duration::ZERO) if empty.
fn compute_percentiles(samples: &VecDeque<Duration>) -> (Duration, Duration, Duration) {
    if samples.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    }

    let mut sorted: Vec<Duration> = samples.iter().copied().collect();
    sorted.sort();

    let len = sorted.len();
    let p50 = sorted[len * 50 / 100];
    let p95 = sorted[(len * 95 / 100).min(len - 1)];
    let p99 = sorted[(len * 99 / 100).min(len - 1)];

    (p50, p95, p99)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metrics(max_batch: u32) -> BatchMetrics {
        BatchMetrics::new(max_batch, MetricsConfig::default())
    }

    #[test]
    fn test_empty_snapshot() {
        let m = make_metrics(16);
        let snap = m.snapshot();
        assert_eq!(snap.avg_batch_size, 0.0);
        assert_eq!(snap.batch_utilization, 0.0);
        assert_eq!(snap.admission_rate, 1.0); // no attempts = no rejections
        assert_eq!(snap.total_steps, 0);
    }

    #[test]
    fn test_record_steps() {
        let m = make_metrics(16);
        m.record_step(4);
        m.record_step(8);
        m.record_step(12);

        let snap = m.snapshot();
        assert!((snap.avg_batch_size - 8.0).abs() < f64::EPSILON);
        assert!((snap.batch_utilization - 0.5).abs() < f64::EPSILON);
        assert_eq!(snap.total_steps, 3);
    }

    #[test]
    fn test_rolling_window_eviction() {
        let config = MetricsConfig {
            window_size: 3,
            max_wait_samples: 100,
        };
        let m = BatchMetrics::new(16, config);

        m.record_step(4);
        m.record_step(8);
        m.record_step(12);
        // Window is [4, 8, 12], avg = 8
        assert!((m.snapshot().avg_batch_size - 8.0).abs() < f64::EPSILON);

        m.record_step(16);
        // Window evicts 4, now [8, 12, 16], avg = 12
        assert!((m.snapshot().avg_batch_size - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_admission_rate() {
        let m = make_metrics(16);
        m.record_admissions(7);
        m.record_rejections(3); // 3 more attempts, no admissions

        let snap = m.snapshot();
        // 7 admitted out of 10 total attempts
        assert!((snap.admission_rate - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_eviction_admission_rate() {
        let m = make_metrics(16);
        m.record_admissions(10);
        m.record_eviction_admissions(3);

        let snap = m.snapshot();
        assert!((snap.eviction_for_admission_rate - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_wait_time_percentiles() {
        let m = make_metrics(16);
        // Insert 100 samples: 1ms, 2ms, ..., 100ms
        for i in 1..=100 {
            m.record_wait_time(Duration::from_millis(i));
        }

        let snap = m.snapshot();
        // P50 should be around 50ms
        assert!(snap.wait_p50.as_millis() >= 49 && snap.wait_p50.as_millis() <= 51);
        // P95 should be around 95ms
        assert!(snap.wait_p95.as_millis() >= 94 && snap.wait_p95.as_millis() <= 96);
        // P99 should be around 99ms
        assert!(snap.wait_p99.as_millis() >= 98 && snap.wait_p99.as_millis() <= 100);
    }

    #[test]
    fn test_completions_tracked() {
        let m = make_metrics(16);
        m.record_completions(5);
        m.record_completions(3);
        assert_eq!(m.snapshot().total_completed, 8);
    }

    #[test]
    fn test_zero_max_batch_size() {
        let m = make_metrics(0);
        m.record_step(5);
        let snap = m.snapshot();
        assert_eq!(snap.batch_utilization, 0.0);
    }

    #[test]
    fn test_wait_time_window_eviction() {
        let config = MetricsConfig {
            window_size: 100,
            max_wait_samples: 5,
        };
        let m = BatchMetrics::new(16, config);

        for i in 1..=10 {
            m.record_wait_time(Duration::from_millis(i * 100));
        }

        // Only last 5 should remain: 600, 700, 800, 900, 1000
        let inner = m.inner.lock().unwrap();
        assert_eq!(inner.wait_times.len(), 5);
        assert_eq!(inner.wait_times[0], Duration::from_millis(600));
    }
}
