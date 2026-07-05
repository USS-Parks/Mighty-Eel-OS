//! Scorer plugins (soft preferences). S2 ships the utilisation scorer; later
//! prompts add the budget/ROI (S5) and spread/HA (S6) scorers.

use crate::framework::Scorer;
use crate::types::{NodeSnapshot, ScheduleRequest};

/// Prefer the least-loaded node: the score is the node's mean free-capacity
/// fraction (`allocatable / capacity`) across the dimensions it actually
/// declares — cpu, memory, gpu, and workload slots. Higher = more headroom =
/// preferred, and it is normalised to `[0.0, 1.0]` so it composes with the
/// other scorers.
///
/// Fail-closed on absent signal (doctrine I-4): a node that declares no total
/// capacity in any dimension gives the scorer nothing real to measure, so it
/// **abstains** (`None`) rather than inventing a fraction. Abstaining does not
/// exclude the node — the readiness and capacity filters own exclusion.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtilizationScorer;

impl Scorer for UtilizationScorer {
    fn name(&self) -> &'static str {
        "utilization"
    }

    // Capacity counts are small (cpu-millis, MB, GPU/slot counts) — far under
    // f64's 2^53 exact-integer range — so the fraction is computed exactly in
    // practice.
    #[allow(clippy::cast_precision_loss)]
    fn score(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> Option<f64> {
        let dims = [
            (node.capacity.cpu_millis, node.allocatable.cpu_millis),
            (node.capacity.memory_mb, node.allocatable.memory_mb),
            (
                u64::from(node.capacity.gpu),
                u64::from(node.allocatable.gpu),
            ),
            (
                u64::from(node.capacity.max_workloads),
                u64::from(node.allocatable.max_workloads),
            ),
        ];
        let mut sum = 0.0;
        let mut declared = 0u32;
        for (total, free) in dims {
            if total == 0 {
                continue;
            }
            sum += (free as f64 / total as f64).clamp(0.0, 1.0);
            declared += 1;
        }
        if declared == 0 {
            return None;
        }
        Some(sum / f64::from(declared))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{AttestationProfile, Capacity, WorkloadKind};
    use fabric_contracts::Classification;

    fn snap(total: Capacity, free: Capacity) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready: true,
            capacity: total,
            allocatable: free,
            last_heartbeat: Some("t".to_owned()),
            resource_version: 1,
        }
    }

    fn req() -> ScheduleRequest {
        ScheduleRequest {
            workload_name: "wl".to_owned(),
            workload_kind: WorkloadKind::Gateway,
            ring: 1,
            classification_ceiling: Classification::Public,
        }
    }

    #[test]
    fn less_loaded_scores_higher() {
        let total = Capacity {
            cpu_millis: 1000,
            memory_mb: 1000,
            gpu: 0,
            max_workloads: 10,
        };
        let idle = snap(
            total,
            Capacity {
                cpu_millis: 900,
                memory_mb: 900,
                gpu: 0,
                max_workloads: 9,
            },
        );
        let busy = snap(
            total,
            Capacity {
                cpu_millis: 100,
                memory_mb: 100,
                gpu: 0,
                max_workloads: 1,
            },
        );
        let idle_score = UtilizationScorer.score(&req(), &idle).expect("idle scores");
        let busy_score = UtilizationScorer.score(&req(), &busy).expect("busy scores");
        assert!(idle_score > busy_score);
    }

    #[test]
    fn undeclared_capacity_abstains() {
        // No declared total in any dimension → no real signal → abstain (None).
        let node = snap(Capacity::default(), Capacity::default());
        assert!(UtilizationScorer.score(&req(), &node).is_none());
    }

    #[test]
    fn fully_free_scores_one() {
        let total = Capacity {
            cpu_millis: 100,
            memory_mb: 100,
            gpu: 4,
            max_workloads: 4,
        };
        let score = UtilizationScorer
            .score(&req(), &snap(total, total))
            .expect("scores");
        assert!((score - 1.0).abs() < 1e-9);
    }
}
