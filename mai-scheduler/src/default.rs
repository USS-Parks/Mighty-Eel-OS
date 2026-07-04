//! DefaultScheduler: the production implementation of the Scheduler trait.
//!
//! Composes the InstanceRegistry, PlacementEngine, and AliasResolver into
//! a single Scheduler implementation. All methods take `&self` and use
//! interior mutability for thread safety.
//!
//! # Concurrency Model
//!
//! - Registry reads (find_by_model, scoring) use DashMap's lock-free reads.
//! - Registry writes (register, remove, metric updates) use per-entry locks.
//! - Alias resolution uses RwLock (read-heavy, write-rare).
//! - Placement scoring holds no locks (operates on cloned snapshots).
//! - ClusterMetrics and routing counters use std::sync::atomic.
//!
//! Multiple concurrent `schedule()` calls proceed in parallel without
//! contention unless they're writing to the same instance's metrics
//! (which is a DashMap per-entry lock, not a global lock).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::aliases::AliasResolver;
use crate::batch::{BatchBuilder, BatchConfig};
use crate::kv::manager::KvCacheManager;
use crate::placement::PlacementEngine;
use crate::registry::InstanceRegistry;
use crate::scheduler::Scheduler;
use crate::scoring::{ScoringConfig, build_multi_factor_scorer_with_reason};
use crate::topology::GpuTopology;
use crate::types::{
    ClusterMetrics, GpuId, InstanceConfig, InstanceId, ScheduleDecision, ScheduleRequest,
    SchedulerConfig, SchedulerError, ScoringFn, SequenceId,
};

/// The production scheduler. Implements the `Scheduler` trait and is stored
/// as `Arc<dyn Scheduler>` in AppState.
pub struct DefaultScheduler {
    /// Instance registry (DashMap-backed, concurrent).
    registry: InstanceRegistry,
    /// Placement engine (scoring + candidate selection).
    placement: PlacementEngine,
    /// Model alias resolver.
    aliases: AliasResolver,
    /// Scheduler configuration.
    config: SchedulerConfig,
    /// GPU topology for hardware-aware placement.
    topology: Option<Arc<GpuTopology>>,
    /// KV cache manager for VRAM-aware placement.
    kv_manager: Option<Arc<dyn KvCacheManager>>,
    /// Per-instance batch builders. Keyed by instance ID.
    /// Each builder is behind a Mutex since `build_step()` needs `&mut self`.
    batch_builders: DashMap<InstanceId, Mutex<BatchBuilder>>,
    /// Batch configuration template for new instances.
    batch_config: BatchConfig,
    total_routed: AtomicU64,
    total_rejected: AtomicU64,
    scoring_config: Option<ScoringConfig>,
}

impl DefaultScheduler {
    /// Create a new DefaultScheduler from configuration.
    pub fn new(config: SchedulerConfig) -> Self {
        let placement = PlacementEngine::new(config.overload_queue_threshold);
        let aliases = AliasResolver::from_config(config.aliases.clone());

        info!(
            strategy = %config.strategy,
            overload_threshold = config.overload_queue_threshold,
            max_queue = config.max_total_queue_depth,
            alias_count = aliases.count(),
            "DefaultScheduler initialized"
        );

        Self {
            registry: InstanceRegistry::new(),
            placement,
            aliases,
            config,
            topology: None,
            kv_manager: None,
            batch_builders: DashMap::new(),
            batch_config: BatchConfig::default(),
            total_routed: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
            scoring_config: None,
        }
    }

    /// Create a scheduler with GPU topology for hardware-aware placement.
    pub fn with_topology(config: SchedulerConfig, topology: Arc<GpuTopology>) -> Self {
        let mut placement = PlacementEngine::new(config.overload_queue_threshold);
        placement.set_topology(Arc::clone(&topology));
        let aliases = AliasResolver::from_config(config.aliases.clone());

        info!(
            strategy = %config.strategy,
            gpu_count = topology.gpu_count(),
            "DefaultScheduler initialized with GPU topology"
        );

        let mut sched = Self {
            registry: InstanceRegistry::new(),
            placement,
            aliases,
            config,
            topology: Some(topology),
            kv_manager: None,
            batch_builders: DashMap::new(),
            batch_config: BatchConfig::default(),
            total_routed: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
            scoring_config: None,
        };
        sched.rebuild_scorer();
        sched
    }

    /// Set the KV cache manager for VRAM-aware placement.
    ///
    /// When set, the scheduler:
    /// - Checks `can_fit()` before placement decisions
    /// - Includes eviction cost in instance scoring
    /// - Calls `touch()` on routed sequences
    /// - Calls `deallocate()` on released sequences
    #[allow(clippy::cast_precision_loss)] // Acceptable: display-only metric
    pub fn set_kv_manager(&mut self, kv_manager: Arc<dyn KvCacheManager>) {
        info!(
            budget_gb = kv_manager.total_bytes() as f64 / 1_000_000_000.0,
            "KV cache manager attached to scheduler"
        );
        self.kv_manager = Some(kv_manager);
        self.rebuild_scorer();
    }

    /// Access the KV cache manager (if set).
    pub fn kv_manager(&self) -> Option<&Arc<dyn KvCacheManager>> {
        self.kv_manager.as_ref()
    }

    /// Access the alias resolver (for config reload).
    pub fn alias_resolver(&self) -> &AliasResolver {
        &self.aliases
    }

    /// Access the registry (for health endpoint introspection).
    pub fn instance_registry(&self) -> &InstanceRegistry {
        &self.registry
    }

    /// Access the GPU topology (if set).
    pub fn topology(&self) -> Option<&Arc<GpuTopology>> {
        self.topology.as_ref()
    }

    /// Compute topology penalty for a set of GPUs.
    /// Returns 0.0 if topology is not configured.
    pub fn topology_penalty(&self, gpu_ids: &[GpuId]) -> f64 {
        self.placement.topology_penalty(gpu_ids)
    }

    /// Set the batch configuration template. New instances registered after
    /// this call will use the provided config. Existing builders are not
    /// retroactively updated (use per-builder config update methods).
    pub fn set_batch_config(&mut self, config: BatchConfig) {
        self.batch_config = config;
    }

    /// Access a batch builder for an instance. Returns None if the instance
    /// has no builder (not registered or batch system not active).
    ///
    /// The caller must lock the Mutex to call `build_step()`, `enqueue()`, etc.
    pub fn batch_builder(
        &self,
        instance: &InstanceId,
    ) -> Option<dashmap::mapref::one::Ref<'_, InstanceId, Mutex<BatchBuilder>>> {
        self.batch_builders.get(instance)
    }

    /// Set the scoring configuration and rebuild the scorer.
    pub fn set_scoring_config(&mut self, config: ScoringConfig) {
        self.scoring_config = Some(config);
        self.rebuild_scorer();
    }

    /// Directly set a scoring function on the placement engine.
    ///
    /// This bypasses the `MultiFactorScorer` builder and is useful for tests
    /// or custom scoring strategies. Clears the stored `scoring_config`.
    pub fn set_scorer(&mut self, scorer: ScoringFn) {
        self.scoring_config = None;
        self.placement.set_scorer(scorer);
    }

    /// Rebuild the `MultiFactorScorer` from the current config, topology,
    /// and KV manager, then set it on the placement engine.
    fn rebuild_scorer(&mut self) {
        let Some(ref config) = self.scoring_config else {
            return;
        };
        let (scorer, reason_fn) = build_multi_factor_scorer_with_reason(
            config.clone(),
            self.topology.clone(),
            self.kv_manager.clone(),
        );
        self.placement.set_scorer_with_reason(scorer, reason_fn);
    }
}

impl Scheduler for DefaultScheduler {
    fn schedule(&self, request: &ScheduleRequest) -> Result<ScheduleDecision, SchedulerError> {
        // Step 0: Backpressure check
        let total_queue = self.registry.total_queue_depth();
        if total_queue >= self.config.max_total_queue_depth && request.priority as u8 > 0 {
            // Only System priority requests bypass backpressure
            self.total_rejected.fetch_add(1, Ordering::Relaxed);
            return Err(SchedulerError::SystemOverloaded(
                total_queue,
                self.config.max_total_queue_depth,
            ));
        }

        // Step 1: Resolve model alias
        let resolved = self.aliases.resolve(&request.model_alias);
        debug!(
            alias = %request.model_alias,
            model = %resolved.model,
            preferred = ?resolved.preferred_backends,
            "Alias resolved"
        );

        // Step 2: Find candidate instances for the backend model
        let all_instances = self.registry.find_by_model(&resolved.model);

        if all_instances.is_empty() {
            self.total_rejected.fetch_add(1, Ordering::Relaxed);
            return Err(SchedulerError::NoInstanceAvailable(
                request.model_alias.clone(),
            ));
        }

        // Step 3: Apply backend preference ordering.
        // If preferred_backends is specified, partition candidates into
        // preferred and non-preferred. Try preferred first; fall back to
        // non-preferred if no preferred candidate is viable.
        let candidates = if resolved.preferred_backends.is_empty() {
            all_instances
        } else {
            let (mut preferred, fallback): (Vec<_>, Vec<_>) =
                all_instances.into_iter().partition(|(_, state)| {
                    resolved
                        .preferred_backends
                        .contains(&state.config.adapter_type)
                });

            if preferred.is_empty() {
                debug!(
                    model = %resolved.model,
                    "No preferred backend instances, falling back to all"
                );
                fallback
            } else {
                // Include fallbacks at the end (lower priority but available)
                preferred.extend(fallback);
                preferred
            }
        };

        // Step 4: Placement
        let decision = self.placement.place(request, &candidates)?;

        // Step 4.5: KV cache awareness
        // If a KV cache manager is attached, check whether the new sequence
        // fits in the VRAM budget. If not, log a warning. Actual eviction is
        // driven by the trigger system (threshold/emergency), not inline here.
        // For continuation requests, touch the existing sequence.
        if let Some(ref kv) = self.kv_manager {
            if let Some(ref cont_seq) = request.continuation_of {
                // Continuation: refresh the sequence's last-access time
                kv.touch(*cont_seq);
            }

            // Estimate if the new allocation would fit
            let estimated_tokens = (request.prompt_tokens + request.max_tokens) as usize;
            // Use a rough per-token estimate (128 KB default, covers most models)
            let rough_bytes_per_token = 131_072.0_f64;
            if !kv.can_fit(estimated_tokens, rough_bytes_per_token) {
                warn!(
                    session = %request.session_id,
                    tokens = estimated_tokens,
                    free_mb = kv.free_bytes() / 1_000_000,
                    "KV cache VRAM pressure: new sequence may not fit"
                );
            }
        }

        // Step 5: Update instance metrics
        self.registry
            .record_request_start(&decision.instance_id, request.session_id);

        self.total_routed.fetch_add(1, Ordering::Relaxed);

        debug!(
            request_session = %request.session_id,
            instance = %decision.instance_id,
            reason = %decision.placement_reason,
            "Request scheduled"
        );

        Ok(decision)
    }

    fn release_sequence(&self, instance: &InstanceId, seq_id: SequenceId) {
        self.registry.record_request_complete(instance);

        // Deallocate KV cache for this sequence
        if let Some(ref kv) = self.kv_manager {
            kv.deallocate(seq_id);
        }

        debug!(instance = %instance, seq = %seq_id, "Sequence released");
    }

    fn register_instance(&self, config: InstanceConfig) -> Result<(), SchedulerError> {
        // Create a batch builder for this instance
        let batch_builder = BatchBuilder::new(config.model_name.clone(), self.batch_config.clone());
        self.batch_builders
            .insert(config.id.clone(), Mutex::new(batch_builder));

        self.registry.register(config)
    }

    fn remove_instance(&self, instance: &InstanceId) {
        self.batch_builders.remove(instance);
        self.registry.remove(instance);
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn cluster_metrics(&self) -> ClusterMetrics {
        let instances = self.registry.list_all();
        let total_instances = instances.len() as u32;
        let total_active: u32 = instances
            .iter()
            .map(|(_, s)| s.metrics.active_sequences)
            .sum();
        let total_queue: u32 = instances.iter().map(|(_, s)| s.metrics.queue_depth).sum();

        let (topo_gpus, topo_cliques) = if let Some(topo) = &self.topology {
            (topo.gpu_count() as u32, topo.nvlink_cliques().len() as u32)
        } else {
            (0, 0)
        };

        // Aggregate batch metrics from all builders
        let mut batch_size_sum = 0.0_f64;
        let mut batch_util_sum = 0.0_f64;
        let mut batch_waiting_total = 0_u32;
        let mut batch_admission_rate_sum = 0.0_f64;
        let mut batch_builder_count = 0_u32;

        for entry in &self.batch_builders {
            if let Ok(builder) = entry.value().lock() {
                let snap = builder.metrics().snapshot();
                batch_size_sum += snap.avg_batch_size;
                batch_util_sum += snap.batch_utilization;
                batch_admission_rate_sum += snap.admission_rate;
                batch_waiting_total += builder.waiting_queue_depth();
                batch_builder_count += 1;
            }
        }

        let avg_batch_size = if batch_builder_count > 0 {
            batch_size_sum / f64::from(batch_builder_count)
        } else {
            0.0
        };
        let avg_batch_utilization = if batch_builder_count > 0 {
            batch_util_sum / f64::from(batch_builder_count)
        } else {
            0.0
        };
        let batch_admission_rate = if batch_builder_count > 0 {
            batch_admission_rate_sum / f64::from(batch_builder_count)
        } else {
            1.0
        };

        ClusterMetrics {
            total_instances,
            healthy_instances: total_instances, // Follow-up: health integration
            total_active_sequences: total_active,
            total_queue_depth: total_queue,
            total_requests_routed: self.total_routed.load(Ordering::Relaxed),
            total_requests_rejected: self.total_rejected.load(Ordering::Relaxed),
            avg_routing_latency_us: 0, // Follow-up: latency tracking
            topology_gpu_count: topo_gpus,
            topology_nvlink_cliques: topo_cliques,
            topology_has_anomalies: false, // Follow-up: wire to MetricsRefresher
            kv_active_sequences: self
                .kv_manager
                .as_ref()
                .map_or(0, |kv| kv.active_sequences() as u32),
            kv_used_bytes: self
                .kv_manager
                .as_ref()
                .map_or(0, |kv| kv.total_bytes() - kv.free_bytes()),
            kv_total_bytes: self.kv_manager.as_ref().map_or(0, |kv| kv.total_bytes()),
            avg_batch_size,
            avg_batch_utilization,
            total_batch_waiting: batch_waiting_total,
            batch_admission_rate,
        }
    }
    // -----------------------------------------------------------------------
    // Power state integration
    // -----------------------------------------------------------------------

    fn can_demote(&self, instance: &InstanceId) -> bool {
        self.registry
            .get(instance)
            .is_some_and(|state| state.metrics.active_sequences == 0)
    }

    fn all_gpu_set(&self) -> Vec<GpuId> {
        let mut gpus = HashSet::new();
        for entry in &self.registry.list_all() {
            for gpu in &entry.1.config.gpu_ids {
                gpus.insert(*gpu);
            }
        }
        gpus.into_iter().collect()
    }

    fn instances_on_gpu(&self, gpu_id: GpuId) -> Vec<InstanceId> {
        self.registry
            .find_by_gpu(gpu_id)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    fn on_wake_gpu(&self, gpu_id: GpuId) -> Result<(), SchedulerError> {
        let instances = self.registry.find_by_gpu(gpu_id);
        if instances.is_empty() {
            return Err(SchedulerError::InstanceNotFound(InstanceId::new(format!(
                "gpu:{gpu_id}"
            ))));
        }
        // Mark all instances on this GPU as healthy by resetting their metrics.
        // In a full implementation, the adapter layer would re-register instances
        // after GPU wake. Here we ensure the scheduler's view is consistent.
        for (id, _) in &instances {
            self.registry.reset_metrics(id);
            info!(instance = %id, gpu = %gpu_id, "Instance marked healthy after GPU wake");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::ScoringConfig;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, ModelAlias, Priority, ScheduleRequest,
        SchedulerConfig, ScoringFn, SequenceId,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    fn test_config() -> SchedulerConfig {
        let mut aliases = HashMap::new();
        aliases.insert(
            "lamprey/fast".to_string(),
            ModelAlias {
                model: "llama3-8b".to_string(),
                preferred_backends: vec!["ollama".to_string(), "vllm".to_string()],
            },
        );
        aliases.insert(
            "lamprey/reason".to_string(),
            ModelAlias {
                model: "qwen3-70b".to_string(),
                preferred_backends: vec!["vllm".to_string()],
            },
        );
        aliases.insert(
            "lamprey/embed".to_string(),
            ModelAlias {
                model: "nomic-embed-text".to_string(),
                preferred_backends: vec!["ollama".to_string()],
            },
        );
        SchedulerConfig {
            strategy: "least-loaded".to_string(),
            overload_queue_threshold: 32,
            max_total_queue_depth: 256,
            aliases,
        }
    }

    fn make_instance(id: &str, model: &str, adapter: &str) -> InstanceConfig {
        InstanceConfig {
            id: InstanceId::new(id),
            model_name: model.to_string(),
            adapter_type: adapter.to_string(),
            gpu_ids: vec![GpuId::new(0)],
            max_batch_size: 16,
            vram_allocated: 8_000_000_000,
            capabilities: InstanceCapabilities::default(),
        }
    }

    #[test]
    fn test_schedule_basic() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();

        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_schedule_unknown_alias_passthrough() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "raw-model", "ollama"))
            .unwrap();

        // No alias for "raw-model", should passthrough
        let req = ScheduleRequest::new("raw-model", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_schedule_no_instance_error() {
        let sched = DefaultScheduler::new(test_config());
        // No instances registered
        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let result = sched.schedule(&req);
        assert!(matches!(
            result,
            Err(SchedulerError::NoInstanceAvailable(_))
        ));
    }

    #[test]
    fn test_schedule_prefers_least_loaded() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        sched
            .register_instance(make_instance("vllm:0", "llama3-8b", "vllm"))
            .unwrap();

        // Directly load up ollama:0 via the registry so we control which
        // instance carries the load (schedule() would distribute across both).
        let ollama_id = InstanceId::new("ollama:0");
        for _ in 0..5 {
            sched
                .registry
                .record_request_start(&ollama_id, SequenceId::new());
        }

        // Next request should go to vllm:0 (less loaded)
        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("vllm:0"));
    }

    #[test]
    fn test_schedule_preferred_backend() {
        let sched = DefaultScheduler::new(test_config());

        // lamprey/reason prefers vllm
        sched
            .register_instance(make_instance("ollama:0", "qwen3-70b", "ollama"))
            .unwrap();
        sched
            .register_instance(make_instance("vllm:0", "qwen3-70b", "vllm"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/reason", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        // vllm is preferred, both have zero load, vllm should be first in partition
        assert_eq!(decision.instance_id, InstanceId::new("vllm:0"));
    }

    #[test]
    fn test_release_decrements() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_queue_depth, 1);

        sched.release_sequence(&decision.instance_id, req.session_id);

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_queue_depth, 0);
    }

    #[test]
    fn test_register_and_remove() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_instances, 1);

        sched.remove_instance(&InstanceId::new("ollama:0"));

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_instances, 0);
    }

    #[test]
    fn test_duplicate_register_error() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        let result = sched.register_instance(make_instance("ollama:0", "other", "ollama"));
        assert!(matches!(result, Err(SchedulerError::DuplicateInstance(_))));
    }

    #[test]
    fn test_continuation_affinity() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        sched
            .register_instance(make_instance("vllm:0", "llama3-8b", "vllm"))
            .unwrap();

        // First request goes to one instance
        let req1 = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision1 = sched.schedule(&req1).unwrap();
        let first_instance = decision1.instance_id.clone();
        sched.release_sequence(&first_instance, req1.session_id);

        // Second request with continuation_of pointing at first session
        let mut req2 = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        req2.continuation_of = Some(req1.session_id);
        let decision2 = sched.schedule(&req2).unwrap();

        // Should go to the same instance (affinity)
        assert_eq!(decision2.instance_id, first_instance);
        assert_eq!(decision2.placement_reason, "continuation-affinity");
    }

    #[test]
    fn test_backpressure_rejects_non_system() {
        let mut config = test_config();
        config.max_total_queue_depth = 2;
        let sched = DefaultScheduler::new(config);

        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // Fill to max
        for _ in 0..2 {
            let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
            sched.schedule(&req).unwrap();
        }

        // Normal priority should be rejected
        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let result = sched.schedule(&req);
        assert!(matches!(
            result,
            Err(SchedulerError::SystemOverloaded(_, _))
        ));
    }

    #[test]
    fn test_backpressure_allows_system() {
        let mut config = test_config();
        config.max_total_queue_depth = 2;
        let sched = DefaultScheduler::new(config);

        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // Fill to max
        for _ in 0..2 {
            let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
            sched.schedule(&req).unwrap();
        }

        // System priority should still work
        let req = ScheduleRequest::new("lamprey/fast", Priority::System);
        let result = sched.schedule(&req);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cluster_metrics() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        sched
            .register_instance(make_instance("vllm:0", "qwen3-70b", "vllm"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        sched.schedule(&req).unwrap();

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_instances, 2);
        assert_eq!(metrics.total_active_sequences, 1);
        assert_eq!(metrics.total_requests_routed, 1);
        assert_eq!(metrics.total_requests_rejected, 0);
    }

    #[test]
    fn test_remove_makes_instance_unroutable() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // Works before remove
        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        assert!(sched.schedule(&req).is_ok());
        sched.release_sequence(&InstanceId::new("ollama:0"), req.session_id);

        // Remove
        sched.remove_instance(&InstanceId::new("ollama:0"));

        // Should fail now
        let req2 = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        assert!(matches!(
            sched.schedule(&req2),
            Err(SchedulerError::NoInstanceAvailable(_))
        ));
    }

    #[test]
    fn test_with_topology() {
        use crate::topology::{GpuTopology, TopologyConfig};

        let config = test_config();
        let topo_config = TopologyConfig::default();
        let topo = Arc::new(GpuTopology::flat(&topo_config));
        let sched = DefaultScheduler::with_topology(config, topo);

        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));

        // Topology should be accessible
        assert!(sched.topology().is_some());
        assert_eq!(sched.topology().unwrap().gpu_count(), 1);

        // Cluster metrics should include topology info
        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.topology_gpu_count, 1);
    }

    #[test]
    fn test_topology_penalty_method() {
        use crate::topology::collector::{LinkType, ParsedGpu, ParsedLink, ParsedTopology};
        use crate::topology::graph::GpuGraph;
        use crate::topology::{GpuTopology, LinkWeightConfig, TopologyConfig};

        let parsed = ParsedTopology {
            gpus: vec![
                ParsedGpu {
                    gpu_id: GpuId(0),
                    name: "GPU0".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(1),
                    name: "GPU1".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(2),
                    name: "GPU2".into(),
                    cpu_affinity: Some(32),
                },
            ],
            links: vec![
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(1),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(0),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(2),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(0),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(2),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(1),
                    link_type: LinkType::SYS,
                },
            ],
            cpu_affinity: [(GpuId(0), 0), (GpuId(1), 0), (GpuId(2), 32)]
                .into_iter()
                .collect(),
        };
        let graph = GpuGraph::from_parsed(&parsed, &LinkWeightConfig::default(), 1.0, 1.0);
        let topo = Arc::new(GpuTopology::from_graph(graph, TopologyConfig::default()));

        let config = test_config();
        let sched = DefaultScheduler::with_topology(config, topo);

        // NVLink pair should have lower penalty
        let nv_penalty = sched.topology_penalty(&[GpuId(0), GpuId(1)]);
        let sys_penalty = sched.topology_penalty(&[GpuId(0), GpuId(2)]);
        assert!(nv_penalty < sys_penalty);
    }

    // Concurrency test: 100 parallel schedule() calls
    #[test]
    fn test_concurrent_scheduling() {
        use std::sync::Arc;
        use std::thread;

        let sched = Arc::new(DefaultScheduler::new(test_config()));
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        sched
            .register_instance(make_instance("vllm:0", "llama3-8b", "vllm"))
            .unwrap();

        let mut handles = Vec::new();
        for i in 0..100 {
            let sched = Arc::clone(&sched);
            handles.push(thread::spawn(move || {
                let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
                let result = sched.schedule(&req);
                // All should succeed (we have plenty of capacity)
                assert!(result.is_ok(), "thread {i} failed: {result:?}");
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.total_requests_routed, 100);
        // Queue depth should be 100 (no releases)
        assert_eq!(metrics.total_queue_depth, 100);
    }

    fn make_kv_manager(budget: u64) -> Arc<crate::kv::HeuristicKvCacheManager> {
        use crate::kv::KvCacheConfig;
        let config = KvCacheConfig {
            total_budget_bytes: budget,
            ..KvCacheConfig::default()
        };
        Arc::new(crate::kv::HeuristicKvCacheManager::new(config))
    }

    #[test]
    fn test_kv_manager_attachment() {
        let mut sched = DefaultScheduler::new(test_config());
        assert!(sched.kv_manager().is_none());

        let kv = make_kv_manager(8_000_000_000);
        sched.set_kv_manager(kv);
        assert!(sched.kv_manager().is_some());
    }

    #[test]
    fn test_cluster_metrics_with_kv() {
        let mut sched = DefaultScheduler::new(test_config());
        let kv = make_kv_manager(10_000_000_000);

        // Allocate a sequence directly in the KV manager
        use crate::kv::sequence::{ModelMemoryFactor, SequenceMeta};
        let factor = ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        };
        let seq = SequenceMeta::new(
            SequenceId::new(),
            InstanceId::new("ollama:0"),
            "llama3-8b".to_string(),
            512,
            Priority::Normal,
            &factor,
        );
        let expected_bytes = seq.kv_bytes;
        kv.allocate(seq).unwrap();

        sched.set_kv_manager(kv);
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.kv_active_sequences, 1);
        assert_eq!(metrics.kv_used_bytes, expected_bytes);
        assert_eq!(metrics.kv_total_bytes, 10_000_000_000);
    }

    #[test]
    fn test_cluster_metrics_no_kv() {
        // Without KV manager, KV fields should be zero
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let metrics = sched.cluster_metrics();
        assert_eq!(metrics.kv_active_sequences, 0);
        assert_eq!(metrics.kv_used_bytes, 0);
        assert_eq!(metrics.kv_total_bytes, 0);
    }

    #[test]
    fn test_release_sequence_deallocates_kv() {
        let mut sched = DefaultScheduler::new(test_config());
        let kv = make_kv_manager(8_000_000_000);

        use crate::kv::sequence::{ModelMemoryFactor, SequenceMeta};
        let factor = ModelMemoryFactor {
            layers: 32,
            kv_heads: 8,
            head_dim: 128,
            dtype_size: 2,
        };
        let seq_id = SequenceId::new();
        let inst_id = InstanceId::new("ollama:0");
        let seq = SequenceMeta::new(
            seq_id,
            inst_id.clone(),
            "llama3-8b".to_string(),
            256,
            Priority::Normal,
            &factor,
        );
        kv.allocate(seq).unwrap();
        assert_eq!(kv.active_sequences(), 1);

        sched.set_kv_manager(kv.clone());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // Release should deallocate from KV
        sched.release_sequence(&inst_id, seq_id);
        assert_eq!(kv.active_sequences(), 0);
        assert_eq!(kv.free_bytes(), 8_000_000_000);
    }

    #[test]
    fn test_kv_can_fit_with_budget() {
        let kv = make_kv_manager(1_000_000); // 1 MB budget
        // llama3-8b per-token = 131,072 bytes. 8 tokens = ~1MB, should just barely fit
        assert!(kv.can_fit(7, 131_072.0));
        // 100 tokens = ~13 MB, should not fit
        assert!(!kv.can_fit(100, 131_072.0));
    }

    #[test]
    fn test_batch_builder_created_on_register() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let builder_ref = sched.batch_builder(&InstanceId::new("ollama:0"));
        assert!(builder_ref.is_some());
        let binding = builder_ref.unwrap();
        let builder = binding.value().lock().unwrap();
        assert_eq!(builder.model(), "llama3-8b");
        assert_eq!(builder.max_batch_size(), 16); // default
    }

    #[test]
    fn test_batch_builder_removed_on_unregister() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        assert!(sched.batch_builder(&InstanceId::new("ollama:0")).is_some());
        sched.remove_instance(&InstanceId::new("ollama:0"));
        assert!(sched.batch_builder(&InstanceId::new("ollama:0")).is_none());
    }

    #[test]
    fn test_cluster_metrics_batch_fields() {
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let metrics = sched.cluster_metrics();
        // No batch activity yet, defaults
        assert_eq!(metrics.total_batch_waiting, 0);
        assert_eq!(metrics.avg_batch_size, 0.0);
        // Admission rate defaults to 1.0 when no attempts
        assert!((metrics.batch_admission_rate - 1.0).abs() < f64::EPSILON);
    }

    fn three_gpu_test_topology() -> Arc<crate::topology::GpuTopology> {
        use crate::topology::collector::{LinkType, ParsedGpu, ParsedLink, ParsedTopology};
        use crate::topology::graph::GpuGraph;
        use crate::topology::{GpuTopology, LinkWeightConfig, TopologyConfig};

        let parsed = ParsedTopology {
            gpus: vec![
                ParsedGpu {
                    gpu_id: GpuId(0),
                    name: "GPU0".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(1),
                    name: "GPU1".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(2),
                    name: "GPU2".into(),
                    cpu_affinity: Some(32),
                },
            ],
            links: vec![
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(1),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(0),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(2),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(0),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(2),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(1),
                    link_type: LinkType::SYS,
                },
            ],
            cpu_affinity: [(GpuId(0), 0), (GpuId(1), 0), (GpuId(2), 32)]
                .into_iter()
                .collect(),
        };
        let graph = GpuGraph::from_parsed(&parsed, &LinkWeightConfig::default(), 1.0, 1.0);
        Arc::new(GpuTopology::from_graph(graph, TopologyConfig::default()))
    }

    fn register_topology_pair(sched: &DefaultScheduler) {
        let mut nvlink_instance = make_instance("inst-nvlink", "llama3-8b", "ollama");
        nvlink_instance.gpu_ids = vec![GpuId(0), GpuId(1)];
        nvlink_instance.vram_allocated = 16_000_000_000;

        let mut sys_instance = make_instance("inst-sys", "llama3-8b", "vllm");
        sys_instance.gpu_ids = vec![GpuId(0), GpuId(2)];
        sys_instance.vram_allocated = 16_000_000_000;

        sched.register_instance(sys_instance).unwrap();
        sched.register_instance(nvlink_instance).unwrap();
    }

    #[test]
    fn test_scorer_wired_with_scoring_config() {
        let mut sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let config = ScoringConfig::default();
        sched.set_scoring_config(config);

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_set_scoring_config_rebuilds_scorer() {
        let mut sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let config = ScoringConfig::default();
        sched.set_scoring_config(config);

        // Second call should rebuild without panic
        let config2 = ScoringConfig {
            latency_weight: 5.0,
            ..ScoringConfig::default()
        };
        sched.set_scoring_config(config2);

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_custom_scorer_bypasses_build() {
        let mut sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // A custom scorer that always returns a high score
        let custom: ScoringFn = Box::new(|_state, _request| 42.0);
        sched.set_scorer(custom);

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_rebuild_scorer_called_after_kv_manager() {
        use crate::kv::HeuristicKvCacheManager;
        use crate::kv::KvCacheConfig;
        let kv_config = KvCacheConfig {
            total_budget_bytes: 8_000_000_000,
            ..KvCacheConfig::default()
        };
        let kv = Arc::new(HeuristicKvCacheManager::new(kv_config));

        let mut sched = DefaultScheduler::new(test_config());
        sched.set_scoring_config(ScoringConfig::default());

        // set_kv_manager should trigger rebuild_scorer internally
        sched.set_kv_manager(kv);

        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_rebuild_scorer_called_with_topology() {
        use crate::topology::{GpuTopology, TopologyConfig};
        let topo = Arc::new(GpuTopology::flat(&TopologyConfig::default()));
        let mut sched = DefaultScheduler::with_topology(test_config(), topo);
        sched.set_scoring_config(ScoringConfig::default());

        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
        assert!(sched.topology().is_some());
    }

    #[test]
    fn test_scoring_config_none_defaults() {
        // Without any scoring config, the default least-loaded strategy applies
        let sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }

    #[test]
    fn test_scorer_affects_placement_order() {
        let mut sched = DefaultScheduler::new(test_config());
        let inst_a = make_instance("inst-a", "llama3-8b", "ollama");
        let inst_b = make_instance("inst-b", "llama3-8b", "vllm");
        sched.register_instance(inst_a).unwrap();
        sched.register_instance(inst_b).unwrap();

        // Make inst-a heavily loaded so the default scorer would avoid it
        let id_a = InstanceId::new("inst-a");
        for _ in 0..100 {
            sched
                .registry
                .record_request_start(&id_a, SequenceId::new());
        }

        // Set scoring config — the multi-factor scorer should penalize inst-a
        sched.set_scoring_config(ScoringConfig::default());

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        // Should pick inst-b (less loaded)
        assert_eq!(decision.instance_id, InstanceId::new("inst-b"));
    }

    #[test]
    fn test_schedule_multi_factor_prefers_lower_topology_cost() {
        let mut sched = DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        sched.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 10.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&sched);

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();

        assert_eq!(decision.instance_id, InstanceId::new("inst-nvlink"));
        assert_eq!(decision.assigned_gpus, vec![GpuId(0), GpuId(1)]);
        assert!(decision.placement_reason.starts_with("multi-factor("));
    }

    #[test]
    fn test_schedule_multi_factor_with_topology_and_kv_handles() {
        use crate::kv::{HeuristicKvCacheManager, KvCacheConfig};
        use crate::topology::{GpuTopology, TopologyConfig};

        let topo = Arc::new(GpuTopology::flat(&TopologyConfig::default()));
        let kv_config = KvCacheConfig {
            total_budget_bytes: 4_000_000_000,
            ..KvCacheConfig::default()
        };
        let kv = Arc::new(HeuristicKvCacheManager::new(kv_config));

        let mut sched = DefaultScheduler::with_topology(test_config(), topo);
        sched.set_kv_manager(kv);
        sched.set_scoring_config(ScoringConfig::default());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        let metrics = sched.cluster_metrics();

        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
        assert_eq!(metrics.total_requests_routed, 1);
        assert_eq!(metrics.topology_gpu_count, 1);
        assert_eq!(metrics.kv_total_bytes, 4_000_000_000);
    }

    #[test]
    fn test_session_19f_schedule_pipeline_eight_scenarios() {
        use crate::kv::{HeuristicKvCacheManager, KvCacheConfig};

        let mut full = DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        let kv = Arc::new(HeuristicKvCacheManager::new(KvCacheConfig {
            total_budget_bytes: 4_000_000_000,
            ..KvCacheConfig::default()
        }));
        full.set_kv_manager(kv);
        full.set_scoring_config(ScoringConfig::default());
        register_topology_pair(&full);

        let first = full
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert!(first.placement_reason.starts_with("multi-factor("));
        assert!(first.placement_reason.contains("lat="));
        assert!(first.placement_reason.contains("mem="));
        assert!(first.placement_reason.contains("topo="));
        assert!(first.placement_reason.contains("evict="));
        assert!(first.placement_reason.contains("batch=-"));

        let metrics = full.cluster_metrics();
        assert_eq!(metrics.topology_gpu_count, 3);
        assert_eq!(metrics.kv_total_bytes, 4_000_000_000);

        let mut topology_only =
            DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        topology_only.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 10.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&topology_only);
        let topology_decision = topology_only
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert_eq!(
            topology_decision.instance_id,
            InstanceId::new("inst-nvlink")
        );

        let mut memory_only =
            DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        memory_only.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 10.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&memory_only);
        memory_only.instance_registry().update_metrics(
            &InstanceId::new("inst-nvlink"),
            |metrics| {
                metrics.vram_used = 15_000_000_000;
            },
        );
        let memory_decision = memory_only
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert_eq!(memory_decision.instance_id, InstanceId::new("inst-sys"));

        let mut latency_only =
            DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        latency_only.set_scoring_config(ScoringConfig {
            latency_weight: 10.0,
            memory_weight: 0.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&latency_only);
        latency_only.instance_registry().update_metrics(
            &InstanceId::new("inst-nvlink"),
            |metrics| {
                metrics.queue_depth = 20;
                metrics.active_sequences = 20;
            },
        );
        let latency_decision = latency_only
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert_eq!(latency_decision.instance_id, InstanceId::new("inst-sys"));

        let mut batching_only =
            DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        batching_only.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 10.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&batching_only);
        batching_only
            .instance_registry()
            .update_metrics(&InstanceId::new("inst-sys"), |metrics| {
                metrics.batch_utilization = 0.95;
                metrics.batch_waiting_count = 128;
                metrics.decode_slots_used = 16;
            });
        let batching_decision = batching_only
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert_eq!(
            batching_decision.instance_id,
            InstanceId::new("inst-nvlink")
        );

        let mut eviction_only = DefaultScheduler::new(test_config());
        eviction_only.set_kv_manager(Arc::new(HeuristicKvCacheManager::new(KvCacheConfig {
            total_budget_bytes: 1,
            ..KvCacheConfig::default()
        })));
        eviction_only.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 0.0,
            eviction_weight: 10.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        eviction_only
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();
        let eviction_decision = eviction_only
            .schedule(&ScheduleRequest {
                prompt_tokens: 1024,
                max_tokens: 1024,
                ..ScheduleRequest::new("lamprey/fast", Priority::Normal)
            })
            .unwrap();
        assert!(eviction_decision.placement_reason.contains("evict=1.000"));

        let mut overload_config = test_config();
        overload_config.overload_queue_threshold = 1;
        let mut overload =
            DefaultScheduler::with_topology(overload_config, three_gpu_test_topology());
        overload.set_scoring_config(ScoringConfig {
            latency_weight: 10.0,
            memory_weight: 0.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        register_topology_pair(&overload);
        overload
            .instance_registry()
            .update_metrics(&InstanceId::new("inst-nvlink"), |metrics| {
                metrics.queue_depth = 2;
            });
        overload
            .instance_registry()
            .update_metrics(&InstanceId::new("inst-sys"), |metrics| {
                metrics.queue_depth = 8;
            });
        let overload_decision = overload
            .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
            .unwrap();
        assert_eq!(
            overload_decision.instance_id,
            InstanceId::new("inst-nvlink")
        );

        let mut rebuild = DefaultScheduler::with_topology(test_config(), three_gpu_test_topology());
        register_topology_pair(&rebuild);
        rebuild.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 0.0,
            topology_weight: 10.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        assert_eq!(
            rebuild
                .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
                .unwrap()
                .instance_id,
            InstanceId::new("inst-nvlink")
        );
        rebuild
            .instance_registry()
            .update_metrics(&InstanceId::new("inst-nvlink"), |metrics| {
                metrics.vram_used = 15_000_000_000;
            });
        rebuild.set_scoring_config(ScoringConfig {
            latency_weight: 0.0,
            memory_weight: 10.0,
            topology_weight: 0.0,
            eviction_weight: 0.0,
            batching_weight: 0.0,
            continuation_bonus: 0.0,
            ..ScoringConfig::default()
        });
        assert_eq!(
            rebuild
                .schedule(&ScheduleRequest::new("lamprey/fast", Priority::Normal))
                .unwrap()
                .instance_id,
            InstanceId::new("inst-sys")
        );
    }

    #[test]
    fn test_set_scorer_overrides_scoring_config() {
        let mut sched = DefaultScheduler::new(test_config());
        sched
            .register_instance(make_instance("ollama:0", "llama3-8b", "ollama"))
            .unwrap();

        // First set a scoring config
        sched.set_scoring_config(ScoringConfig::default());

        // Then override with a custom scorer
        let custom: ScoringFn = Box::new(|_state, _request| 42.0);
        sched.set_scorer(custom);

        let req = ScheduleRequest::new("lamprey/fast", Priority::Normal);
        let decision = sched.schedule(&req).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("ollama:0"));
    }
}
