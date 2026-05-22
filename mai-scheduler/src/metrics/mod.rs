//! Metrics collection, health scoring, and anomaly detection for the scheduler.
//!
//! # Components
//!
//! - `MetricsCollector`: top-level public interface, stored as `Arc<MetricsCollector>`
//! - `RingBuffer<T>`: generic ring buffer with configurable capacity
//! - `FeedbackProcessor`: processes request completions
//! - `InstanceHealthTracker`: per-instance health scoring
//! - `AnomalyDetector`: per-instance anomaly detection
//! - `RequestLifecycle`: per-request timing and prediction error

pub mod anomaly;
pub mod feedback;
pub mod health;
pub mod lifecycle;
pub mod store;

use serde::{Deserialize, Serialize};

use dashmap::DashMap;

use crate::types::{InstanceId, InstanceMetrics};

pub use anomaly::{AnomalyConfig, AnomalyEvent, AnomalyKind};
pub use feedback::{CompletionReport, FeedbackConfig, FeedbackProcessor};
pub use health::{HealthConfig, HealthScore};
pub use lifecycle::{LifecycleConfig, RequestLifecycle};
pub use store::RingBuffer;

/// Top-level configuration for the metrics subsystem.
/// Loaded from config/metrics.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    #[serde(default)]
    pub feedback: FeedbackConfig,
    #[serde(default = "default_max_anomaly_events")]
    pub max_anomaly_events: usize,
}

fn default_max_anomaly_events() -> usize {
    1000
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            feedback: FeedbackConfig::default(),
            max_anomaly_events: default_max_anomaly_events(),
        }
    }
}

/// Public interface for the metrics system.
/// Stored as `Arc<MetricsCollector>` in AppState.
pub struct MetricsCollector {
    processor: FeedbackProcessor,
    config: MetricsConfig,
}

impl MetricsCollector {
    pub fn new(config: MetricsConfig) -> Self {
        let processor = FeedbackProcessor::new(config.feedback.clone());
        Self { processor, config }
    }

    /// Record a request completion. Called from the scheduler's release_sequence path.
    /// Accepts an `&InstanceMetrics` snapshot and returns the updated metrics.
    pub fn record_completion(
        &self,
        report: CompletionReport,
        current_metrics: &InstanceMetrics,
    ) -> InstanceMetrics {
        self.processor.process_completion(report, current_metrics)
    }

    /// Get health scores for all instances that have received requests.
    pub fn all_health_scores(&self, registry_metrics: &DashMap<InstanceId, InstanceMetrics>) -> Vec<(InstanceId, HealthScore)> {
        let mut scores = Vec::new();
        for entry in registry_metrics {
            let id = entry.key().clone();
            let metrics = entry.value();
            if let Some(score) = self.processor.health_score(&id, metrics) {
                scores.push((id, score));
            }
        }
        scores
    }

    /// Get health score for a specific instance.
    pub fn instance_health_score(&self, instance_id: &InstanceId, metrics: &InstanceMetrics) -> Option<HealthScore> {
        self.processor.health_score(instance_id, metrics)
    }

    /// Get recent anomaly events.
    pub fn recent_anomalies(&self) -> Vec<AnomalyEvent> {
        self.processor.recent_anomalies()
    }

    /// Get prediction error for an instance.
    pub fn prediction_error(&self, _instance_id: &InstanceId) -> Option<f64> {
        self.processor.lifecycle.rolling_prediction_error()
    }

    /// Get recent request lifecycles.
    pub fn recent_lifecycles(&self) -> Vec<RequestLifecycle> {
        self.processor.lifecycle.recent()
    }

    pub fn config(&self) -> &MetricsConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SequenceId;

    fn make_collector() -> MetricsCollector {
        MetricsCollector::new(MetricsConfig::default())
    }

    fn report(instance_id: &str, latency: u64) -> CompletionReport {
        CompletionReport {
            session_id: SequenceId::new(),
            instance_id: InstanceId::new(instance_id),
            scheduled_at: 1000,
            predicted_latency_ms: latency,
            actual_latency_ms: latency,
            first_token_at: Some(1000 + latency),
            tokens_generated: 100,
            is_error: false,
            vram_used_after: 4_000_000_000,
            tokens_per_sec: 200.0,
        }
    }

    #[test]
    fn test_collector_record_completion() {
        let collector = make_collector();
        let metrics = InstanceMetrics::default();
        let updated = collector.record_completion(report("i:0", 50), &metrics);
        assert_eq!(updated.queue_depth, 0);
        assert_eq!(collector.recent_lifecycles().len(), 1);
    }

    #[test]
    fn test_health_score_query() {
        let collector = make_collector();
        let id = InstanceId::new("i:0");
        let metrics = InstanceMetrics::default();
        for _ in 0..10 {
            collector.record_completion(report("i:0", 50), &metrics);
        }
        let score = collector.instance_health_score(&id, &metrics);
        assert!(score.is_some());
        assert!(score.unwrap().overall > 0.8);
    }

    #[test]
    fn test_anomalies_query() {
        let collector = make_collector();
        let metrics = InstanceMetrics::default();
        for _ in 0..10 {
            collector.record_completion(report("i:0", 50), &metrics);
        }
        collector.record_completion(
            CompletionReport {
                actual_latency_ms: 500,
                ..report("i:0", 500)
            },
            &metrics,
        );
        let anomalies = collector.recent_anomalies();
        assert!(!anomalies.is_empty());
    }

    #[test]
    fn test_prediction_error() {
        let collector = make_collector();
        let metrics = InstanceMetrics::default();
        collector.record_completion(report("i:0", 50), &metrics);
        let err = collector.prediction_error(&InstanceId::new("i:0"));
        assert!(err.is_some());
    }

    #[test]
    fn test_all_health_scores_empty_registry() {
        let collector = make_collector();
        let registry = DashMap::new();
        let scores = collector.all_health_scores(&registry);
        assert!(scores.is_empty());
    }

    #[test]
    fn test_all_health_scores_with_data() {
        let collector = make_collector();
        let registry = DashMap::new();
        let id = InstanceId::new("i:0");
        let metrics = InstanceMetrics::default();
        registry.insert(id.clone(), metrics.clone());
        for _ in 0..10 {
            collector.record_completion(report("i:0", 50), &metrics);
        }
        let scores = collector.all_health_scores(&registry);
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].0, id);
    }
}
