use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::types::{InstanceId, InstanceMetrics, SequenceId};

use super::health::{HealthConfig, InstanceHealthTracker};
use super::lifecycle::{LifecycleConfig, PerInstanceLifecycle, RequestLifecycle};
use super::anomaly::{AnomalyConfig, AnomalyDetector, AnomalyEvent, AnomalyKind};

/// Configuration for the feedback processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackConfig {
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub anomaly: AnomalyConfig,
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleConfig::default(),
            health: HealthConfig::default(),
            anomaly: AnomalyConfig::default(),
        }
    }
}

/// Summary of a request completion, fed into the feedback processor.
pub struct CompletionReport {
    pub session_id: SequenceId,
    pub instance_id: InstanceId,
    pub scheduled_at: u64,
    pub predicted_latency_ms: u64,
    pub actual_latency_ms: u64,
    pub first_token_at: Option<u64>,
    pub tokens_generated: u32,
    pub is_error: bool,
    pub vram_used_after: u64,
    pub tokens_per_sec: f64,
}

/// Processes request completions and updates lifecycle tracking,
/// health scoring, and anomaly detection.
pub struct FeedbackProcessor {
    pub lifecycle: PerInstanceLifecycle,
    health_trackers: dashmap::DashMap<InstanceId, InstanceHealthTracker>,
    anomaly_detectors: dashmap::DashMap<InstanceId, AnomalyDetector>,
    anomaly_events: Mutex<Vec<AnomalyEvent>>,
    config: FeedbackConfig,
}

impl FeedbackProcessor {
    pub fn new(config: FeedbackConfig) -> Self {
        Self {
            lifecycle: PerInstanceLifecycle::new(config.lifecycle.window_size),
            health_trackers: dashmap::DashMap::new(),
            anomaly_detectors: dashmap::DashMap::new(),
            anomaly_events: Mutex::new(Vec::new()),
            config,
        }
    }

    /// Process a request completion. Updates all tracking subsystems.
    /// Returns the updated instance metrics if the instance was tracked.
    pub fn process_completion(
        &self,
        report: CompletionReport,
        current_metrics: &InstanceMetrics,
    ) -> InstanceMetrics {
        // Record lifecycle
        let lifecycle = RequestLifecycle {
            session_id: report.session_id,
            instance_id: report.instance_id.clone(),
            scheduled_at: report.scheduled_at,
            dispatched_at: None,
            first_token_at: report.first_token_at,
            completed_at: Some(0), // not used for now
            tokens_generated: Some(report.tokens_generated),
            predicted_latency_ms: report.predicted_latency_ms,
            actual_latency_ms: Some(report.actual_latency_ms),
            is_error: report.is_error,
        };
        self.lifecycle.record(lifecycle);

        // Update health tracking
        let mut health = self.health_trackers
            .entry(report.instance_id.clone())
            .or_insert_with(|| InstanceHealthTracker::new(&self.config.health));
        health.record_completion(
            report.actual_latency_ms,
            report.is_error,
            report.vram_used_after,
            report.tokens_per_sec,
        );

        // Update anomaly detection
        let mut anomaly = self.anomaly_detectors
            .entry(report.instance_id.clone())
            .or_insert_with(|| AnomalyDetector::new(self.config.anomaly.clone()));
        anomaly.record(report.actual_latency_ms, report.vram_used_after, report.tokens_per_sec);

        // Check for anomalies
        let now = report.actual_latency_ms + report.scheduled_at;
        if let Some(severity) = anomaly.check_latency_spike() {
            if let Ok(mut events) = self.anomaly_events.lock() {
                events.push(AnomalyEvent {
                    instance_id: report.instance_id.clone(),
                    kind: AnomalyKind::LatencySpike,
                    severity,
                    timestamp: now,
                    description: format!("Latency spike detected: ratio={severity:.2}"),
                });
            }
        }
        if let Some(severity) = anomaly.check_memory_leak() {
            if let Ok(mut events) = self.anomaly_events.lock() {
                events.push(AnomalyEvent {
                    instance_id: report.instance_id.clone(),
                    kind: AnomalyKind::MemoryLeak,
                    severity,
                    timestamp: now,
                    description: format!("Memory leak detected: slope={severity:.4}"),
                });
            }
        }
        if let Some(severity) = anomaly.check_throughput_drop() {
            if let Ok(mut events) = self.anomaly_events.lock() {
                events.push(AnomalyEvent {
                    instance_id: report.instance_id.clone(),
                    kind: AnomalyKind::ThroughputDrop,
                    severity,
                    timestamp: now,
                    description: format!("Throughput drop detected: drop_ratio={severity:.2}"),
                });
            }
        }

        // Compute updated metrics
        let _health_score = health.score(current_metrics, &self.config.health);
        let mut updated = current_metrics.clone();
        updated.queue_depth = current_metrics.queue_depth.saturating_sub(1);
        if report.is_error {
            // Don't decrement active_sequences if it was an error (adapter may still have it)
        } else {
            updated.active_sequences = current_metrics.active_sequences.saturating_sub(1);
        }

        updated
    }

    /// Get the health score for an instance.
    pub fn health_score(&self, instance_id: &InstanceId, metrics: &InstanceMetrics) -> Option<crate::metrics::health::HealthScore> {
        self.health_trackers
            .get(instance_id)
            .map(|t| t.score(metrics, &self.config.health))
    }

    /// Drain recent anomaly events.
    pub fn drain_anomalies(&self) -> Vec<AnomalyEvent> {
        let mut events = self.anomaly_events.lock().unwrap();
        std::mem::take(&mut *events)
    }

    pub fn recent_anomalies(&self) -> Vec<AnomalyEvent> {
        self.anomaly_events.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn report(instance_id: &str, latency: u64, is_error: bool) -> CompletionReport {
        CompletionReport {
            session_id: SequenceId::new(),
            instance_id: InstanceId::new(instance_id),
            scheduled_at: 1000,
            predicted_latency_ms: latency,
            actual_latency_ms: latency,
            first_token_at: Some(1000 + latency),
            tokens_generated: 100,
            is_error,
            vram_used_after: 4_000_000_000,
            tokens_per_sec: 200.0,
        }
    }

    #[test]
    fn test_process_completion_updates_lifecycle() {
        let processor = FeedbackProcessor::new(FeedbackConfig::default());
        let metrics = InstanceMetrics::default();
        let _updated = processor.process_completion(report("i:0", 50, false), &metrics);
        let recent = processor.lifecycle.recent();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].actual_latency_ms, Some(50));
    }

    #[test]
    fn test_health_score_available() {
        let processor = FeedbackProcessor::new(FeedbackConfig::default());
        let metrics = InstanceMetrics::default();
        let id = InstanceId::new("i:0");
        for _ in 0..10 {
            processor.process_completion(report("i:0", 50, false), &metrics);
        }
        let score = processor.health_score(&id, &metrics);
        assert!(score.is_some());
        assert!(score.unwrap().overall > 0.8);
    }

    #[test]
    fn test_anomalies_recorded() {
        let processor = FeedbackProcessor::new(FeedbackConfig::default());
        let metrics = InstanceMetrics::default();
        // Normal completions
        for _ in 0..10 {
            processor.process_completion(report("i:0", 50, false), &metrics);
        }
        // Spike
        processor.process_completion(
            CompletionReport {
                actual_latency_ms: 500,
                ..report("i:0", 500, false)
            },
            &metrics,
        );
        let anomalies = processor.recent_anomalies();
        let has_spike = anomalies.iter().any(|a| a.kind == AnomalyKind::LatencySpike);
        assert!(has_spike, "should have latency spike anomaly");
    }

    #[test]
    fn test_updated_metrics_queue_decrement() {
        let processor = FeedbackProcessor::new(FeedbackConfig::default());
        let mut metrics = InstanceMetrics::default();
        metrics.queue_depth = 5;
        metrics.active_sequences = 3;
        let updated = processor.process_completion(report("i:0", 50, false), &metrics);
        assert_eq!(updated.queue_depth, 4);
        assert_eq!(updated.active_sequences, 2);
    }

    #[test]
    fn test_drain_anomalies() {
        let processor = FeedbackProcessor::new(FeedbackConfig::default());
        let metrics = InstanceMetrics::default();
        for _ in 0..10 {
            processor.process_completion(report("i:0", 50, false), &metrics);
        }
        processor.process_completion(
            CompletionReport {
                actual_latency_ms: 500,
                ..report("i:0", 500, false)
            },
            &metrics,
        );
        let drained = processor.drain_anomalies();
        assert!(!drained.is_empty());
        assert!(processor.recent_anomalies().is_empty());
    }
}
