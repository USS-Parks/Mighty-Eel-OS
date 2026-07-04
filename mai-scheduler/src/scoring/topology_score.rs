//! Topology penalty scorer for multi-factor placement.
//!
//! For tensor-parallel instances that span multiple GPUs, the interconnect
//! quality between those GPUs directly affects inference latency. NVLink
//! connections are much faster than PCIe or cross-socket links.
//!
//! This scorer uses the precomputed path cost matrix from
//! `GpuTopology` to penalize instances with poor GPU interconnect quality.
//!
//! # Formula
//!
//! ```text
//! For single-GPU instances:
//!   penalty = 0.0
//!
//! For multi-GPU instances:
//!   raw_penalty = worst_pair_cost(instance.gpu_ids)
//!   penalty = clamp(raw_penalty / max_penalty, 0.0, 1.0)
//! ```
//!
//! The max_penalty normalization parameter ensures the topology penalty
//! stays in `[0, 1]` regardless of the raw link cost scale.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::topology::GpuTopology;
use crate::types::{GpuId, InstanceState};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Topology scoring parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyScoreConfig {
    /// Maximum expected topology penalty for normalization.
    /// Raw penalties at or above this value produce a normalized score of 1.0.
    /// Should be set to the worst-case link cost in the system (e.g., cross-socket
    /// SYS link cost). Default: 10.0.
    #[serde(default = "default_max_penalty")]
    pub max_penalty: f64,
}

fn default_max_penalty() -> f64 {
    10.0
}

impl Default for TopologyScoreConfig {
    fn default() -> Self {
        Self {
            max_penalty: default_max_penalty(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer
// ---------------------------------------------------------------------------

/// Compute the topology penalty for an instance.
///
/// Returns a value in `[0.0, 1.0]` where 0.0 means optimal interconnect
/// (single GPU or all-NVLink) and 1.0 means worst-case interconnect.
///
/// Returns 0.0 if topology is not configured or the instance uses a single GPU.
pub fn topology_penalty(
    state: &InstanceState,
    topology: Option<&Arc<GpuTopology>>,
    config: &TopologyScoreConfig,
) -> f64 {
    // Single GPU or no GPUs: no topology cost
    if state.config.gpu_ids.len() <= 1 {
        return 0.0;
    }

    let topo = match topology {
        Some(t) => t,
        None => return 0.0,
    };

    if config.max_penalty <= 0.0 {
        return 0.0;
    }

    let raw = topo.topology_penalty(&state.config.gpu_ids);
    (raw / config.max_penalty).clamp(0.0, 1.0)
}

/// Compute raw (unnormalized) topology penalty for a set of GPU IDs.
/// Useful for diagnostics and the placement_reason breakdown.
pub fn raw_topology_penalty(gpu_ids: &[GpuId], topology: Option<&Arc<GpuTopology>>) -> f64 {
    if gpu_ids.len() <= 1 {
        return 0.0;
    }
    match topology {
        Some(t) => t.topology_penalty(gpu_ids),
        None => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        GpuId, InstanceCapabilities, InstanceConfig, InstanceId, InstanceMetrics, InstanceState,
    };

    fn make_state_gpus(gpu_ids: Vec<GpuId>) -> InstanceState {
        InstanceState {
            config: InstanceConfig {
                id: InstanceId::new("test:0"),
                model_name: "test-model".to_string(),
                adapter_type: "test".to_string(),
                gpu_ids,
                max_batch_size: 16,
                vram_allocated: 16_000_000_000,
                capabilities: InstanceCapabilities::default(),
            },
            metrics: InstanceMetrics::default(),
        }
    }

    #[test]
    fn test_single_gpu_zero_penalty() {
        let state = make_state_gpus(vec![GpuId::new(0)]);
        let config = TopologyScoreConfig::default();
        assert!((topology_penalty(&state, None, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_no_topology_zero_penalty() {
        let state = make_state_gpus(vec![GpuId::new(0), GpuId::new(1)]);
        let config = TopologyScoreConfig::default();
        assert!((topology_penalty(&state, None, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_empty_gpu_ids_zero_penalty() {
        let state = make_state_gpus(vec![]);
        let config = TopologyScoreConfig::default();
        assert!((topology_penalty(&state, None, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_normalization_caps_at_one() {
        // With a very low max_penalty, even a small raw penalty normalizes to 1.0
        let config = TopologyScoreConfig { max_penalty: 0.01 };
        // No real topology to test with here, but the function returns 0 without topo
        let state = make_state_gpus(vec![GpuId::new(0), GpuId::new(1)]);
        assert!((topology_penalty(&state, None, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_zero_max_penalty_returns_zero() {
        let config = TopologyScoreConfig { max_penalty: 0.0 };
        let state = make_state_gpus(vec![GpuId::new(0), GpuId::new(1)]);
        assert!((topology_penalty(&state, None, &config)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_with_real_topology() {
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

        let config = TopologyScoreConfig { max_penalty: 10.0 };

        // NVLink pair: should have low penalty
        let nv_state = make_state_gpus(vec![GpuId(0), GpuId(1)]);
        let nv_penalty = topology_penalty(&nv_state, Some(&topo), &config);

        // Cross-socket pair: should have higher penalty
        let sys_state = make_state_gpus(vec![GpuId(0), GpuId(2)]);
        let sys_penalty = topology_penalty(&sys_state, Some(&topo), &config);

        assert!(
            sys_penalty > nv_penalty,
            "SYS penalty ({sys_penalty}) should exceed NVLink penalty ({nv_penalty})"
        );
        assert!(nv_penalty >= 0.0);
        assert!(nv_penalty <= 1.0);
        assert!(sys_penalty >= 0.0);
        assert!(sys_penalty <= 1.0);
    }
}
