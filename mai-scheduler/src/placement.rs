//! Placement engine: decides which instance handles a request.
//!
//! Phase 1: least-loaded placement with continuation affinity.
//! Phase 2: topology_penalty method added for hardware-aware
//! placement. NOT wired into the default scorer yet.
//! The scoring function is pluggable via `ScoringFn`, so can
//! replace it with a multi-factor scorer without changing the placement
//! engine's structure.
//!
//! # Placement Algorithm
//!
//! 1. Resolve model alias to backend model name + preferred backends
//! 2. Find candidate instances that serve the backend model
//! 3. Filter: skip overloaded instances (queue_depth >= threshold)
//! 4. Continuation affinity: if `continuation_of` is set, check if the
//!    previous instance is still a candidate. If so, prefer it.
//! 5. Score remaining candidates with the pluggable scoring function
//! 6. Return the lowest-scored candidate as the placement decision

use std::sync::Arc;

use tracing::{debug, warn};

use crate::topology::GpuTopology;
use crate::types::{
    GpuId, InstanceId, InstanceState, ScheduleDecision, ScheduleRequest, SchedulerError, ScoringFn,
    ScoringReasonFn,
};

/// The placement engine. Holds references to the registry and alias resolver,
/// plus the pluggable scoring function.
pub struct PlacementEngine {
    /// Scoring function. Lower score = better candidate.
    scoring_fn: ScoringFn,
    /// Optional score breakdown formatter for diagnostics.
    scoring_reason_fn: Option<ScoringReasonFn>,
    /// Queue depth threshold for overload filtering.
    overload_threshold: u32,
    /// GPU topology for hardware-aware placement scoring.
    /// None until topology is initialized.
    topology: Option<Arc<GpuTopology>>,
}

impl PlacementEngine {
    /// Create a new placement engine with the default least-loaded scorer.
    pub fn new(overload_threshold: u32) -> Self {
        Self {
            scoring_fn: Box::new(least_loaded_scorer),
            scoring_reason_fn: None,
            overload_threshold,
            topology: None,
        }
    }

    /// Create a placement engine with a custom scoring function.
    pub fn with_scorer(overload_threshold: u32, scorer: ScoringFn) -> Self {
        Self {
            scoring_fn: scorer,
            scoring_reason_fn: None,
            overload_threshold,
            topology: None,
        }
    }

    /// Replace the scoring function at runtime.
    pub fn set_scorer(&mut self, scorer: ScoringFn) {
        self.scoring_fn = scorer;
        self.scoring_reason_fn = None;
    }

    /// Replace the scoring function and its diagnostic formatter at runtime.
    pub fn set_scorer_with_reason(&mut self, scorer: ScoringFn, reason_fn: ScoringReasonFn) {
        self.scoring_fn = scorer;
        self.scoring_reason_fn = Some(reason_fn);
    }

    /// Set the GPU topology for hardware-aware placement.
    pub fn set_topology(&mut self, topology: Arc<GpuTopology>) {
        self.topology = Some(topology);
    }

    /// Compute topology penalty for an instance's GPU assignment.
    ///
    /// Returns the worst-case interconnect cost between assigned GPUs.
    /// Higher penalty means worse interconnect quality for tensor-parallel
    /// workloads. Returns 0.0 if topology is not set or instance has <= 1 GPU.
    ///
    /// NOT wired into the default scorer yet. integrates this
    /// into the multi-factor scoring function.
    pub fn topology_penalty(&self, gpu_ids: &[GpuId]) -> f64 {
        self.topology
            .as_ref()
            .map_or(0.0, |topo| topo.topology_penalty(gpu_ids))
    }

    /// Execute placement for a request.
    ///
    /// `candidates` is the list of (InstanceId, InstanceState) pairs that
    /// serve the requested model. The placement engine filters, scores,
    /// and selects from these candidates.
    pub fn place(
        &self,
        request: &ScheduleRequest,
        candidates: &[(InstanceId, InstanceState)],
    ) -> Result<ScheduleDecision, SchedulerError> {
        if candidates.is_empty() {
            return Err(SchedulerError::NoInstanceAvailable(
                request.model_alias.clone(),
            ));
        }

        // Step 1: Filter overloaded instances
        let viable: Vec<&(InstanceId, InstanceState)> = candidates
            .iter()
            .filter(|(_, state)| state.metrics.queue_depth < self.overload_threshold)
            .collect();

        // If all candidates are overloaded, still try (degraded service > no service).
        // But log a warning.
        let pool = if viable.is_empty() {
            warn!(
                model = %request.model_alias,
                candidates = candidates.len(),
                threshold = self.overload_threshold,
                "All candidates overloaded, using least-bad option"
            );
            candidates.iter().collect::<Vec<_>>()
        } else {
            viable
        };

        // Step 2: Continuation affinity check
        if let Some(ref continuation_seq) = request.continuation_of {
            for (id, state) in &pool {
                if let Some(ref last_seq) = state.metrics.last_sequence_id
                    && last_seq == continuation_seq
                {
                    debug!(
                        instance = %id,
                        seq = %continuation_seq,
                        "Continuation affinity match"
                    );
                    return Ok(ScheduleDecision {
                        instance_id: id.clone(),
                        assigned_gpus: state.config.gpu_ids.clone(),
                        estimated_latency_ms: estimate_latency(state),
                        placement_reason: "continuation-affinity".to_string(),
                    });
                }
            }
            // No affinity match found; fall through to normal scoring
            debug!(
                seq = %continuation_seq,
                "No continuation affinity match, falling through to scoring"
            );
        }

        // Step 3: Score and select
        let mut best: Option<(&InstanceId, &InstanceState, f64)> = None;

        for (id, state) in &pool {
            let score = (self.scoring_fn)(state, request);
            match &best {
                None => best = Some((id, state, score)),
                Some((_, _, best_score)) if score < *best_score => {
                    best = Some((id, state, score));
                }
                _ => {}
            }
        }

        let (best_id, best_state, _score) =
            best.ok_or_else(|| SchedulerError::NoInstanceAvailable(request.model_alias.clone()))?;

        let reason = if let Some(reason_fn) = &self.scoring_reason_fn {
            reason_fn(best_state, request)
        } else if pool.len() == 1 {
            "only-candidate".to_string()
        } else {
            "least-loaded".to_string()
        };

        Ok(ScheduleDecision {
            instance_id: best_id.clone(),
            assigned_gpus: best_state.config.gpu_ids.clone(),
            estimated_latency_ms: estimate_latency(best_state),
            placement_reason: reason,
        })
    }
}

// ---------------------------------------------------------------------------
// Default scoring function
// ---------------------------------------------------------------------------

/// Phase 1 scorer: prefer lower queue_depth, break ties by lower vram_used.
///
/// Score = queue_depth * 1000.0 + (vram_used / 1_000_000) as f64
///
/// This ensures queue_depth is the primary factor and vram_used is the
/// tiebreaker. replaces this with a multi-factor scorer that
/// includes topology cost, KV cache affinity, thermal headroom, etc.
#[allow(clippy::cast_precision_loss)] // Acceptable: scoring doesn't need full u64 precision
fn least_loaded_scorer(state: &InstanceState, _request: &ScheduleRequest) -> f64 {
    let queue_score = f64::from(state.metrics.queue_depth) * 1000.0;
    let vram_score = (state.metrics.vram_used / 1_000_000) as f64;
    queue_score + vram_score
}

/// Rough latency estimate based on queue depth. 50ms base + 20ms per queued item.
fn estimate_latency(state: &InstanceState) -> u64 {
    50 + u64::from(state.metrics.queue_depth) * 20
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, InstanceMetrics, InstanceState, Priority,
        ScheduleRequest, SequenceId,
    };

    fn make_state(
        id: &str,
        model: &str,
        queue_depth: u32,
        vram_used: u64,
    ) -> (InstanceId, InstanceState) {
        (
            InstanceId::new(id),
            InstanceState {
                config: InstanceConfig {
                    id: InstanceId::new(id),
                    model_name: model.to_string(),
                    adapter_type: "test".to_string(),
                    gpu_ids: vec![GpuId::new(0)],
                    max_batch_size: 16,
                    vram_allocated: 16_000_000_000,
                    capabilities: InstanceCapabilities::default(),
                },
                metrics: InstanceMetrics {
                    queue_depth,
                    active_sequences: queue_depth,
                    vram_used,
                    last_request_epoch_ms: 0,
                    last_sequence_id: None,
                    ..InstanceMetrics::default()
                },
            },
        )
    }

    fn make_request(model: &str) -> ScheduleRequest {
        ScheduleRequest::new(model, Priority::Normal)
    }

    #[test]
    fn test_single_candidate_selected() {
        let engine = PlacementEngine::new(64);
        let candidates = vec![make_state("a:0", "llama3", 0, 0)];
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("a:0"));
        assert_eq!(decision.placement_reason, "only-candidate");
    }

    #[test]
    fn test_least_loaded_wins() {
        let engine = PlacementEngine::new(64);
        let candidates = vec![
            make_state("a:0", "llama3", 5, 0),
            make_state("b:0", "llama3", 2, 0),
            make_state("c:0", "llama3", 8, 0),
        ];
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
        assert_eq!(decision.placement_reason, "least-loaded");
    }

    #[test]
    fn test_vram_tiebreaker() {
        let engine = PlacementEngine::new(64);
        let candidates = vec![
            make_state("a:0", "llama3", 3, 8_000_000_000),
            make_state("b:0", "llama3", 3, 2_000_000_000),
        ];
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        // Both have queue_depth 3, b:0 has lower vram_used
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
    }

    #[test]
    fn test_overloaded_filtered() {
        let engine = PlacementEngine::new(10);
        let candidates = vec![
            make_state("a:0", "llama3", 15, 0), // overloaded
            make_state("b:0", "llama3", 3, 0),  // viable
        ];
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
    }

    #[test]
    fn test_all_overloaded_degrades_gracefully() {
        let engine = PlacementEngine::new(2);
        let candidates = vec![
            make_state("a:0", "llama3", 5, 0),
            make_state("b:0", "llama3", 3, 0),
        ];
        // Both overloaded (threshold=2), but we still pick the least bad
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
    }

    #[test]
    fn test_continuation_affinity() {
        let engine = PlacementEngine::new(64);
        let seq = SequenceId::new();

        let (id_a, mut state_a) = make_state("a:0", "llama3", 5, 0);
        // a:0 last served this sequence
        state_a.metrics.last_sequence_id = Some(seq);

        let candidates = vec![
            (id_a, state_a),
            make_state("b:0", "llama3", 1, 0), // lower load but not affinity match
        ];

        let mut req = make_request("llama3");
        req.continuation_of = Some(seq);

        let decision = engine.place(&req, &candidates).unwrap();
        assert_eq!(decision.instance_id, InstanceId::new("a:0"));
        assert_eq!(decision.placement_reason, "continuation-affinity");
    }

    #[test]
    fn test_continuation_no_match_falls_through() {
        let engine = PlacementEngine::new(64);
        let candidates = vec![
            make_state("a:0", "llama3", 5, 0),
            make_state("b:0", "llama3", 1, 0),
        ];

        let mut req = make_request("llama3");
        req.continuation_of = Some(SequenceId::new()); // no match in candidates

        let decision = engine.place(&req, &candidates).unwrap();
        // Should fall through to normal scoring; b:0 has lower queue depth
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
    }

    #[test]
    fn test_empty_candidates_error() {
        let engine = PlacementEngine::new(64);
        let result = engine.place(&make_request("llama3"), &[]);
        assert!(matches!(
            result,
            Err(SchedulerError::NoInstanceAvailable(_))
        ));
    }

    #[test]
    fn test_custom_scorer() {
        // Custom scorer that prefers higher queue depth (inverse, for testing)
        let engine = PlacementEngine::with_scorer(
            64,
            Box::new(|state: &InstanceState, _req: &ScheduleRequest| {
                -(f64::from(state.metrics.queue_depth))
            }),
        );
        let candidates = vec![
            make_state("a:0", "llama3", 1, 0),
            make_state("b:0", "llama3", 10, 0),
        ];
        let decision = engine.place(&make_request("llama3"), &candidates).unwrap();
        // Custom scorer prefers higher queue depth (lower score = better, and -10 < -1)
        assert_eq!(decision.instance_id, InstanceId::new("b:0"));
    }

    #[test]
    fn test_latency_estimate() {
        let (_, state) = make_state("a:0", "llama3", 5, 0);
        assert_eq!(estimate_latency(&state), 50 + 5 * 20); // 150ms
    }

    #[test]
    fn test_topology_penalty_no_topology() {
        let engine = PlacementEngine::new(64);
        // Without topology, penalty is always 0.0
        assert_eq!(
            engine.topology_penalty(&[GpuId::new(0), GpuId::new(1)]),
            0.0
        );
    }

    #[test]
    fn test_topology_penalty_with_topology() {
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
        let config = TopologyConfig::default();
        let topo = Arc::new(GpuTopology::from_graph(graph, config));

        let mut engine = PlacementEngine::new(64);
        engine.set_topology(topo);

        // NVLink pair should have lower penalty than cross-socket pair
        let nv_penalty = engine.topology_penalty(&[GpuId(0), GpuId(1)]);
        let sys_penalty = engine.topology_penalty(&[GpuId(0), GpuId(2)]);
        assert!(
            nv_penalty < sys_penalty,
            "NV4 penalty ({nv_penalty}) should be less than SYS penalty ({sys_penalty})"
        );

        // Single GPU has zero penalty
        assert_eq!(engine.topology_penalty(&[GpuId(0)]), 0.0);
    }
}
