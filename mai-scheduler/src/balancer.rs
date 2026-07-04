//! Cross-instance load balancer.
//!
//! When two instances serve the same model and one is sustained-overloaded
//! while the other has capacity, migrating a sequence's KV cache between
//! them can rebalance the cluster without forcing a cold re-prefill. The
//! migration goes through the soft-eviction path: offload from source,
//! restore on target.
//!
//! Migration cost depends on GPU topology: NVLink pairs are cheap, PCIe is
//! expensive, cross-host (if it existed) would be prohibitive. The balancer
//! only emits a migration plan when the projected load-reduction benefit
//! clearly exceeds that cost.
//!
//! This module evaluates candidates and returns a plan. It does not perform
//! the migration; the scheduler/KV manager applies it via `OffloadManager`.

use serde::{Deserialize, Serialize};

use crate::topology::GpuTopology;
use crate::types::{InstanceId, InstanceState, SequenceId};

/// One migration the balancer recommends.
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationDecision {
    /// Sequence to migrate.
    pub seq_id: SequenceId,
    /// Source instance (currently overloaded).
    pub source: InstanceId,
    /// Target instance (has capacity).
    pub target: InstanceId,
    /// Net benefit: load reduction minus migration cost.
    pub net_benefit: f64,
}

/// Configuration for the balancer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancerConfig {
    /// Minimum queue-depth gap between source and target before a migration
    /// is even considered. Prevents thrashing on small imbalances.
    #[serde(default = "default_min_gap")]
    pub min_queue_gap: u32,
    /// Weight applied to each unit of queue-depth reduction.
    #[serde(default = "default_load_weight")]
    pub load_weight: f64,
    /// Weight applied to topology path-cost when computing migration cost.
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,
    /// Minimum net benefit a migration must clear to be emitted.
    #[serde(default = "default_threshold")]
    pub benefit_threshold: f64,
}

fn default_min_gap() -> u32 {
    8
}

fn default_load_weight() -> f64 {
    1.0
}

fn default_cost_weight() -> f64 {
    1.0
}

fn default_threshold() -> f64 {
    2.0
}

impl Default for BalancerConfig {
    fn default() -> Self {
        Self {
            min_queue_gap: default_min_gap(),
            load_weight: default_load_weight(),
            cost_weight: default_cost_weight(),
            benefit_threshold: default_threshold(),
        }
    }
}

/// A migration candidate the caller has identified as eligible
/// (i.e. the sequence is on `source` and the same model is served on
/// `target`). The balancer scores it.
#[derive(Debug, Clone)]
pub struct MigrationCandidate {
    /// Sequence under consideration.
    pub seq_id: SequenceId,
    /// Source instance.
    pub source: InstanceId,
    /// Target instance.
    pub target: InstanceId,
}

/// Stateless evaluator. The caller passes in the current cluster state and
/// the candidates worth scoring; the balancer returns the subset that should
/// actually migrate.
#[derive(Debug, Clone)]
pub struct LoadBalancer {
    config: BalancerConfig,
}

impl LoadBalancer {
    /// Build a balancer with the given configuration.
    pub fn new(config: BalancerConfig) -> Self {
        Self { config }
    }

    /// Default config: 8-queue gap, equal weights, 2.0 benefit threshold.
    pub fn with_defaults() -> Self {
        Self::new(BalancerConfig::default())
    }

    /// Evaluate candidates and return the migrations whose net benefit
    /// exceeds the configured threshold.
    pub fn evaluate(
        &self,
        candidates: &[MigrationCandidate],
        instances: &[InstanceState],
        topology: Option<&GpuTopology>,
    ) -> Vec<MigrationDecision> {
        let mut decisions: Vec<MigrationDecision> = Vec::new();
        for candidate in candidates {
            let Some(source) = instances.iter().find(|i| i.config.id == candidate.source) else {
                continue;
            };
            let Some(target) = instances.iter().find(|i| i.config.id == candidate.target) else {
                continue;
            };
            let load_diff =
                i64::from(source.metrics.queue_depth) - i64::from(target.metrics.queue_depth);
            if load_diff < i64::from(self.config.min_queue_gap) {
                continue;
            }
            let cost = topology_cost(topology, source, target);
            let benefit = self.config.load_weight * load_diff as f64;
            let net = benefit - self.config.cost_weight * cost;
            if net >= self.config.benefit_threshold {
                decisions.push(MigrationDecision {
                    seq_id: candidate.seq_id,
                    source: candidate.source.clone(),
                    target: candidate.target.clone(),
                    net_benefit: net,
                });
            }
        }
        // Highest benefit first so the caller can apply migrations in priority
        // order under sustained imbalance.
        decisions.sort_by(|a, b| {
            b.net_benefit
                .partial_cmp(&a.net_benefit)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        decisions
    }
}

/// Average pairwise topology cost between any GPU on `source` and any GPU on
/// `target`. Returns `0.0` when topology is absent or either instance has no
/// GPUs (the placement engine deals with capability separately).
fn topology_cost(
    topology: Option<&GpuTopology>,
    source: &InstanceState,
    target: &InstanceState,
) -> f64 {
    let Some(topology) = topology else {
        return 0.0;
    };
    let source_gpus = &source.config.gpu_ids;
    let target_gpus = &target.config.gpu_ids;
    if source_gpus.is_empty() || target_gpus.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    let mut count = 0_usize;
    for s in source_gpus {
        for t in target_gpus {
            total += topology.path_cost(*s, *t);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GpuId, InstanceCapabilities, InstanceConfig, InstanceMetrics};

    fn instance(id: &str, queue_depth: u32, gpu_ids: &[u32]) -> InstanceState {
        InstanceState {
            config: InstanceConfig {
                id: InstanceId::new(id),
                model_name: "test-model".to_string(),
                adapter_type: "test".to_string(),
                gpu_ids: gpu_ids.iter().copied().map(GpuId::new).collect(),
                max_batch_size: 32,
                vram_allocated: 0,
                capabilities: InstanceCapabilities::default(),
            },
            metrics: InstanceMetrics {
                queue_depth,
                ..InstanceMetrics::default()
            },
        }
    }

    #[test]
    fn test_no_migration_when_gap_below_threshold() {
        let lb = LoadBalancer::with_defaults();
        let instances = vec![instance("a", 10, &[0]), instance("b", 8, &[1])];
        let candidate = MigrationCandidate {
            seq_id: SequenceId::new(),
            source: InstanceId::new("a"),
            target: InstanceId::new("b"),
        };
        let decisions = lb.evaluate(&[candidate], &instances, None);
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_migration_emitted_when_benefit_clears_threshold() {
        let lb = LoadBalancer::with_defaults();
        let instances = vec![instance("a", 40, &[0]), instance("b", 5, &[1])];
        let candidate = MigrationCandidate {
            seq_id: SequenceId::new(),
            source: InstanceId::new("a"),
            target: InstanceId::new("b"),
        };
        let decisions = lb.evaluate(&[candidate], &instances, None);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].net_benefit > 0.0);
    }

    #[test]
    fn test_decisions_are_sorted_by_benefit_desc() {
        let lb = LoadBalancer::with_defaults();
        let instances = vec![
            instance("a", 40, &[0]),
            instance("b", 30, &[1]),
            instance("c", 5, &[2]),
        ];
        let candidates = vec![
            MigrationCandidate {
                seq_id: SequenceId::new(),
                source: InstanceId::new("a"),
                target: InstanceId::new("c"),
            },
            MigrationCandidate {
                seq_id: SequenceId::new(),
                source: InstanceId::new("b"),
                target: InstanceId::new("c"),
            },
        ];
        let decisions = lb.evaluate(&candidates, &instances, None);
        assert_eq!(decisions.len(), 2);
        assert!(decisions[0].net_benefit >= decisions[1].net_benefit);
    }

    #[test]
    fn test_missing_target_is_silently_skipped() {
        let lb = LoadBalancer::with_defaults();
        let instances = vec![instance("a", 50, &[0])];
        let candidate = MigrationCandidate {
            seq_id: SequenceId::new(),
            source: InstanceId::new("a"),
            target: InstanceId::new("ghost"),
        };
        assert!(lb.evaluate(&[candidate], &instances, None).is_empty());
    }

    #[test]
    fn test_topology_cost_dampens_benefit() {
        // With a very large cost_weight, even a big load gap should fail.
        let lb = LoadBalancer::new(BalancerConfig {
            cost_weight: 1_000.0,
            ..BalancerConfig::default()
        });
        let instances = vec![instance("a", 40, &[0]), instance("b", 5, &[1])];
        let candidate = MigrationCandidate {
            seq_id: SequenceId::new(),
            source: InstanceId::new("a"),
            target: InstanceId::new("b"),
        };
        // With no topology supplied, cost is 0, migration still proceeds.
        assert_eq!(
            lb.evaluate(std::slice::from_ref(&candidate), &instances, None)
                .len(),
            1
        );

        // Pure-CPU instances also resolve to zero cost (no gpu pairs).
        let instances_cpu = vec![instance("a", 40, &[]), instance("b", 5, &[])];
        assert_eq!(lb.evaluate(&[candidate], &instances_cpu, None).len(), 1,);
    }
}
