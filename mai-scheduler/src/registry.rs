//! Thread-safe instance registry.
//!
//! The registry is the scheduler's view of the world: which instances exist,
//! what models they serve, and their live metrics. It is the single source of
//! truth for instance state within the scheduler.
//!
//! Backed by `DashMap` for lock-free concurrent reads and fine-grained write
//! locking. Multiple `schedule()` calls can read the registry simultaneously;
//! registration and removal take per-entry locks, not global locks.

use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tracing::{debug, info, warn};

use crate::types::{
    GpuId, InstanceConfig, InstanceId, InstanceMetrics, InstanceState, SchedulerError, SequenceId,
};

/// Thread-safe registry of active model instances.
///
/// All methods take `&self` and are safe to call concurrently from multiple
/// tokio tasks. Interior mutability is provided by `DashMap`.
pub struct InstanceRegistry {
    /// Active instances keyed by InstanceId.
    instances: DashMap<InstanceId, InstanceState>,
}

impl InstanceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            instances: DashMap::new(),
        }
    }

    /// Register a new instance. Returns error if the ID is already registered.
    pub fn register(&self, config: InstanceConfig) -> Result<(), SchedulerError> {
        let id = config.id.clone();
        let state = InstanceState {
            config,
            metrics: InstanceMetrics::default(),
        };

        // DashMap::entry gives us atomic check-and-insert
        match self.instances.entry(id.clone()) {
            Entry::Occupied(_) => Err(SchedulerError::DuplicateInstance(id)),
            Entry::Vacant(v) => {
                info!(instance = %v.key(), model = %state.config.model_name, "Instance registered");
                v.insert(state);
                Ok(())
            }
        }
    }

    /// Remove an instance by ID. No-op if not found (idempotent for crash paths).
    pub fn remove(&self, id: &InstanceId) {
        if self.instances.remove(id).is_some() {
            info!(instance = %id, "Instance removed from registry");
        } else {
            debug!(instance = %id, "Remove called for unknown instance (no-op)");
        }
    }

    /// Total number of registered instances.
    pub fn count(&self) -> usize {
        self.instances.len()
    }

    /// Find all instances that serve a given backend model name.
    /// Returns (InstanceId, InstanceState) pairs. The caller should not hold
    /// these references across await points.
    pub fn find_by_model(&self, model_name: &str) -> Vec<(InstanceId, InstanceState)> {
        self.instances
            .iter()
            .filter(|entry| entry.value().config.model_name == model_name)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Find all instances running on a specific GPU.
    pub fn find_by_gpu(&self, gpu_id: GpuId) -> Vec<(InstanceId, InstanceState)> {
        self.instances
            .iter()
            .filter(|entry| entry.value().config.gpu_ids.contains(&gpu_id))
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// List all instances with their current state. Used by cluster_metrics()
    /// and health endpoints.
    pub fn list_all(&self) -> Vec<(InstanceId, InstanceState)> {
        self.instances
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Get a snapshot of a single instance's state.
    pub fn get(&self, id: &InstanceId) -> Option<InstanceState> {
        self.instances.get(id).map(|entry| entry.value().clone())
    }

    /// Increment queue_depth and active_sequences for an instance.
    /// Called when the scheduler routes a request to this instance.
    /// Also sets last_request_epoch_ms and last_sequence_id.
    pub fn record_request_start(&self, id: &InstanceId, seq_id: SequenceId) {
        if let Some(mut entry) = self.instances.get_mut(id) {
            let metrics = &mut entry.metrics;
            metrics.queue_depth = metrics.queue_depth.saturating_add(1);
            metrics.active_sequences = metrics.active_sequences.saturating_add(1);
            metrics.last_sequence_id = Some(seq_id);
            #[allow(clippy::cast_possible_truncation)] // Epoch millis fit in u64 until year 584M+
            let epoch_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            metrics.last_request_epoch_ms = epoch_ms;
        } else {
            warn!(instance = %id, "record_request_start for unknown instance");
        }
    }

    /// Decrement queue_depth and active_sequences for an instance.
    /// Called when a request completes (via `release_sequence`).
    pub fn record_request_complete(&self, id: &InstanceId) {
        if let Some(mut entry) = self.instances.get_mut(id) {
            let metrics = &mut entry.metrics;
            metrics.queue_depth = metrics.queue_depth.saturating_sub(1);
            metrics.active_sequences = metrics.active_sequences.saturating_sub(1);
        } else {
            debug!(instance = %id, "record_request_complete for unknown instance (may have been removed)");
        }
    }

    /// Check if an instance is overloaded (queue_depth >= threshold).
    pub fn is_overloaded(&self, id: &InstanceId, threshold: u32) -> bool {
        self.instances
            .get(id)
            .is_none_or(|entry| entry.metrics.queue_depth >= threshold)
    }

    /// Total active sequences across all instances.
    pub fn total_active_sequences(&self) -> u32 {
        self.instances
            .iter()
            .map(|entry| entry.metrics.active_sequences)
            .sum()
    }

    /// Total queue depth across all instances.
    pub fn total_queue_depth(&self) -> u32 {
        self.instances
            .iter()
            .map(|entry| entry.metrics.queue_depth)
            .sum()
    }

    /// Reset metrics for an instance to default values. Used by power management
    /// after GPU wake to clear drained state.
    pub fn reset_metrics(&self, id: &InstanceId) {
        if let Some(mut entry) = self.instances.get_mut(id) {
            entry.metrics = InstanceMetrics::default();
            debug!(instance = %id, "Instance metrics reset (power state transition)");
        }
    }

    /// Mutate an instance's metrics in place.
    ///
    /// Used by health, telemetry, and integration tests to feed observed
    /// runtime measurements into placement scoring.
    pub fn update_metrics(
        &self,
        id: &InstanceId,
        update: impl FnOnce(&mut InstanceMetrics),
    ) -> bool {
        if let Some(mut entry) = self.instances.get_mut(id) {
            update(&mut entry.metrics);
            true
        } else {
            false
        }
    }
}

impl Default for InstanceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::InstanceCapabilities;

    fn make_config(id: &str, model: &str) -> InstanceConfig {
        InstanceConfig {
            id: InstanceId::new(id),
            model_name: model.to_string(),
            adapter_type: "ollama".to_string(),
            gpu_ids: vec![GpuId::new(0)],
            max_batch_size: 16,
            vram_allocated: 8_000_000_000,
            capabilities: InstanceCapabilities {
                context_window: 8192,
                supports_streaming: true,
                supports_batch: true,
                supports_embeddings: false,
                supports_structured: false,
                supports_function_calling: false,
            },
        }
    }

    #[test]
    fn test_register_and_count() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        reg.register(make_config("vllm:0", "qwen3-70b")).unwrap();
        assert_eq!(reg.count(), 2);
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        let result = reg.register(make_config("ollama:0", "different-model"));
        assert!(matches!(result, Err(SchedulerError::DuplicateInstance(_))));
    }

    #[test]
    fn test_remove_and_count() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        assert_eq!(reg.count(), 1);
        reg.remove(&InstanceId::new("ollama:0"));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_remove_unknown_is_noop() {
        let reg = InstanceRegistry::new();
        reg.remove(&InstanceId::new("nonexistent"));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_find_by_model() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        reg.register(make_config("vllm:0", "llama3-8b")).unwrap();
        reg.register(make_config("vllm:1", "qwen3-70b")).unwrap();

        let llama_instances = reg.find_by_model("llama3-8b");
        assert_eq!(llama_instances.len(), 2);

        let qwen_instances = reg.find_by_model("qwen3-70b");
        assert_eq!(qwen_instances.len(), 1);

        let none_instances = reg.find_by_model("nonexistent");
        assert_eq!(none_instances.len(), 0);
    }

    #[test]
    fn test_find_by_gpu() {
        let reg = InstanceRegistry::new();
        let mut cfg = make_config("ollama:0", "llama3-8b");
        cfg.gpu_ids = vec![GpuId::new(0), GpuId::new(1)];
        reg.register(cfg).unwrap();

        let mut cfg2 = make_config("vllm:0", "qwen3-70b");
        cfg2.gpu_ids = vec![GpuId::new(1)];
        reg.register(cfg2).unwrap();

        let gpu0 = reg.find_by_gpu(GpuId::new(0));
        assert_eq!(gpu0.len(), 1);

        let gpu1 = reg.find_by_gpu(GpuId::new(1));
        assert_eq!(gpu1.len(), 2);
    }

    #[test]
    fn test_request_start_and_complete() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        let id = InstanceId::new("ollama:0");

        let seq = SequenceId::new();
        reg.record_request_start(&id, seq);

        let state = reg.get(&id).unwrap();
        assert_eq!(state.metrics.queue_depth, 1);
        assert_eq!(state.metrics.active_sequences, 1);
        assert!(state.metrics.last_request_epoch_ms > 0);
        assert_eq!(state.metrics.last_sequence_id, Some(seq));

        reg.record_request_complete(&id);
        let state = reg.get(&id).unwrap();
        assert_eq!(state.metrics.queue_depth, 0);
        assert_eq!(state.metrics.active_sequences, 0);
    }

    #[test]
    fn test_is_overloaded() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        let id = InstanceId::new("ollama:0");

        assert!(!reg.is_overloaded(&id, 2));

        // Fill to threshold
        reg.record_request_start(&id, SequenceId::new());
        reg.record_request_start(&id, SequenceId::new());
        assert!(reg.is_overloaded(&id, 2));
    }

    #[test]
    fn test_total_metrics() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("ollama:0", "llama3-8b")).unwrap();
        reg.register(make_config("vllm:0", "qwen3-70b")).unwrap();

        let id0 = InstanceId::new("ollama:0");
        let id1 = InstanceId::new("vllm:0");

        reg.record_request_start(&id0, SequenceId::new());
        reg.record_request_start(&id0, SequenceId::new());
        reg.record_request_start(&id1, SequenceId::new());

        assert_eq!(reg.total_active_sequences(), 3);
        assert_eq!(reg.total_queue_depth(), 3);
    }

    #[test]
    fn test_list_all() {
        let reg = InstanceRegistry::new();
        reg.register(make_config("a:0", "m1")).unwrap();
        reg.register(make_config("b:0", "m2")).unwrap();
        assert_eq!(reg.list_all().len(), 2);
    }
}
