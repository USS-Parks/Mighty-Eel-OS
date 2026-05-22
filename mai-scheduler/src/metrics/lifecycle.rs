use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::types::{InstanceId, SequenceId};
use super::store::RingBuffer;

/// Configuration for request lifecycle tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleConfig {
    /// Number of recent lifecycles to retain per instance (ring buffer size).
    #[serde(default = "default_window_size")]
    pub window_size: usize,
}

fn default_window_size() -> usize {
    1000
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            window_size: default_window_size(),
        }
    }
}

/// Tracks the full lifecycle of a single request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLifecycle {
    pub session_id: SequenceId,
    pub instance_id: InstanceId,
    /// Timestamp when the scheduler made the placement decision (epoch ms).
    pub scheduled_at: u64,
    /// Timestamp when the adapter received the request (epoch ms).
    pub dispatched_at: Option<u64>,
    /// Timestamp when the first token was generated (epoch ms).
    pub first_token_at: Option<u64>,
    /// Timestamp when generation finished (epoch ms).
    pub completed_at: Option<u64>,
    /// Actual number of output tokens generated.
    pub tokens_generated: Option<u32>,
    /// Latency predicted by the scheduler at placement time (ms).
    pub predicted_latency_ms: u64,
    /// Actual end-to-end latency: first_token_at - scheduled_at (ms).
    pub actual_latency_ms: Option<u64>,
    /// Whether the request completed with an error.
    pub is_error: bool,
}

impl RequestLifecycle {
    pub fn prediction_error(&self) -> Option<f64> {
        let actual = self.actual_latency_ms?;
        if actual == 0 {
            return None;
        }
        let diff = if self.predicted_latency_ms > actual {
            self.predicted_latency_ms - actual
        } else {
            actual - self.predicted_latency_ms
        };
        Some(diff as f64 / actual as f64)
    }
}

/// Stores recent request lifecycles per instance.
pub struct PerInstanceLifecycle {
    lifecycles: Mutex<RingBuffer<RequestLifecycle>>,
}

impl PerInstanceLifecycle {
    pub fn new(window_size: usize) -> Self {
        Self {
            lifecycles: Mutex::new(RingBuffer::new(window_size)),
        }
    }

    pub fn record(&self, lifecycle: RequestLifecycle) {
        if let Ok(mut buf) = self.lifecycles.lock() {
            buf.push(lifecycle);
        }
    }

    pub fn recent(&self) -> Vec<RequestLifecycle> {
        self.lifecycles.lock().map_or_else(
            |_| Vec::new(),
            |buf| buf.iter().cloned().collect(),
        )
    }

    pub fn rolling_prediction_error(&self) -> Option<f64> {
        let buf = self.lifecycles.lock().ok()?;
        let errors: Vec<f64> = buf.iter().filter_map(|l| l.prediction_error()).collect();
        let count = errors.len();
        if count == 0 {
            return None;
        }
        Some(errors.iter().sum::<f64>() / count as f64)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lifecycle(instance_id: &str, predicted: u64, actual: u64) -> RequestLifecycle {
        RequestLifecycle {
            session_id: SequenceId::new(),
            instance_id: InstanceId::new(instance_id),
            scheduled_at: 1000,
            dispatched_at: Some(1050),
            first_token_at: Some(1000 + actual),
            completed_at: Some(1000 + actual + 500),
            tokens_generated: Some(100),
            predicted_latency_ms: predicted,
            actual_latency_ms: Some(actual),
            is_error: false,
        }
    }

    #[test]
    fn test_prediction_error_formula() {
        let lc = sample_lifecycle("test:0", 100, 80);
        let err = lc.prediction_error().unwrap();
        // |100 - 80| / 80 = 0.25
        assert!((err - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_prediction_error_zero_actual() {
        let lc = RequestLifecycle {
            actual_latency_ms: Some(0),
            ..sample_lifecycle("test:0", 100, 0)
        };
        assert!(lc.prediction_error().is_none());
    }

    #[test]
    fn test_prediction_error_no_actual() {
        let lc = RequestLifecycle {
            actual_latency_ms: None,
            ..sample_lifecycle("test:0", 100, 0)
        };
        assert!(lc.prediction_error().is_none());
    }

    #[test]
    fn test_rolling_prediction_error() {
        let store = PerInstanceLifecycle::new(10);
        store.record(sample_lifecycle("i:0", 100, 80)); // err = 0.25
        store.record(sample_lifecycle("i:0", 200, 100)); // err = 1.0
        let rolling = store.rolling_prediction_error().unwrap();
        // (0.25 + 1.0) / 2 = 0.625
        assert!((rolling - 0.625).abs() < 1e-6);
    }

    #[test]
    fn test_rolling_empty() {
        let store = PerInstanceLifecycle::new(10);
        assert!(store.rolling_prediction_error().is_none());
    }

    #[test]
    fn test_recent_lifecycles() {
        let store = PerInstanceLifecycle::new(3);
        store.record(sample_lifecycle("i:0", 100, 80));
        store.record(sample_lifecycle("i:0", 100, 90));
        assert_eq!(store.recent().len(), 2);
    }

    #[test]
    fn test_window_eviction() {
        let store = PerInstanceLifecycle::new(2);
        store.record(sample_lifecycle("i:0", 100, 80));
        store.record(sample_lifecycle("i:0", 100, 90));
        store.record(sample_lifecycle("i:0", 100, 100));
        assert_eq!(store.recent().len(), 2);
        // Oldest should be evicted (err=0.25 removed), only err=0.111... and err=0.0 remain
        let expected = (100_u64.saturating_sub(90) as f64) / 90.0;
        let first = store.recent()[0].prediction_error().unwrap();
        assert!((first - expected).abs() < 1e-6);
    }
}
