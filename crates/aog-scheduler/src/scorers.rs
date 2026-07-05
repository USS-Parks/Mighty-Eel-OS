//! Scorer plugins (soft preferences). S2 ships the utilisation scorer and S5
//! the consolidation scorer; both read the same real capacity telemetry. S6
//! adds the spread/HA scorer.

use crate::framework::Scorer;
use crate::types::{NodeSnapshot, ScheduleRequest};

/// Mean free-capacity fraction (`allocatable / capacity`) across the dimensions
/// the node actually declares — cpu, memory, gpu, and workload slots — or `None`
/// when it declares no total capacity in any dimension. This is the shared,
/// real-signal basis of the utilisation (spread) and consolidation (pack)
/// scorers; neither invents a value where the node reports none (doctrine I-4).
// Capacity counts are small (cpu-millis, MB, GPU/slot counts) — far under f64's
// 2^53 exact-integer range — so the fraction is computed exactly in practice.
#[allow(clippy::cast_precision_loss)]
fn mean_free_fraction(node: &NodeSnapshot) -> Option<f64> {
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
    (declared > 0).then(|| sum / f64::from(declared))
}

/// Prefer the least-loaded node (spread): the score is the node's mean free
/// fraction, in `[0.0, 1.0]`. Higher = more headroom = preferred. Abstains
/// (`None`) when the node declares no total capacity — no invented fraction.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtilizationScorer;

impl Scorer for UtilizationScorer {
    fn name(&self) -> &'static str {
        "utilization"
    }

    fn score(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> Option<f64> {
        mean_free_fraction(node)
    }
}

/// Prefer consolidation (bin-packing) to reduce the number of active nodes and
/// the hardware bill: the score is the node's mean *used* fraction
/// (`1 - free_fraction`), in `[0.0, 1.0]`. Higher = more already-used = tighter
/// packing. This is the placement-time, real-signal half of the budget/ROI
/// objective (S5); spend-weighted ROI from the meter folds in when the meter
/// feeds per-node efficiency into the estate.
///
/// It is the deliberate counterweight to [`UtilizationScorer`] (spread): an
/// operator composes the two by weight to pick a cost posture (pack) or an HA
/// posture (spread), which is why the default wiring carries spread and leaves
/// consolidation opt-in. Abstains (`None`) when the node declares no total
/// capacity (doctrine I-4).
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsolidationScorer;

impl Scorer for ConsolidationScorer {
    fn name(&self) -> &'static str {
        "consolidation"
    }

    fn score(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> Option<f64> {
        mean_free_fraction(node).map(|free| 1.0 - free)
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
    fn utilization_prefers_the_idler() {
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
    fn utilization_abstains_without_capacity() {
        let node = snap(Capacity::default(), Capacity::default());
        assert!(UtilizationScorer.score(&req(), &node).is_none());
    }

    #[test]
    fn utilization_fully_free_scores_one() {
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

    #[test]
    fn consolidation_prefers_the_fuller_node() {
        let total = Capacity {
            cpu_millis: 100,
            memory_mb: 100,
            gpu: 0,
            max_workloads: 10,
        };
        let full = snap(
            total,
            Capacity {
                cpu_millis: 10,
                memory_mb: 10,
                gpu: 0,
                max_workloads: 1,
            },
        );
        let empty = snap(
            total,
            Capacity {
                cpu_millis: 90,
                memory_mb: 90,
                gpu: 0,
                max_workloads: 9,
            },
        );
        let full_score = ConsolidationScorer.score(&req(), &full).expect("scores");
        let empty_score = ConsolidationScorer.score(&req(), &empty).expect("scores");
        assert!(full_score > empty_score);
    }

    #[test]
    fn consolidation_is_deterministic_from_fixture() {
        // Fixed telemetry: cpu 1/4 free, mem 1/2 free, slots 0/1 free (gpu
        // undeclared → skipped). Mean free (0.25 + 0.5 + 0.0)/3 = 0.25 → used
        // 0.75. Exact and repeatable — no clock, no RNG.
        let total = Capacity {
            cpu_millis: 4,
            memory_mb: 2,
            gpu: 0,
            max_workloads: 1,
        };
        let free = Capacity {
            cpu_millis: 1,
            memory_mb: 1,
            gpu: 0,
            max_workloads: 0,
        };
        let score = ConsolidationScorer
            .score(&req(), &snap(total, free))
            .expect("scores");
        assert!((score - 0.75).abs() < 1e-9);
    }

    #[test]
    fn utilization_and_consolidation_are_complementary() {
        let total = Capacity {
            cpu_millis: 8,
            memory_mb: 8,
            gpu: 2,
            max_workloads: 4,
        };
        let free = Capacity {
            cpu_millis: 2,
            memory_mb: 4,
            gpu: 1,
            max_workloads: 1,
        };
        let node = snap(total, free);
        let util = UtilizationScorer.score(&req(), &node).expect("util");
        let cons = ConsolidationScorer.score(&req(), &node).expect("cons");
        assert!((util + cons - 1.0).abs() < 1e-9);
    }

    #[test]
    fn consolidation_abstains_without_capacity() {
        let node = snap(Capacity::default(), Capacity::default());
        assert!(ConsolidationScorer.score(&req(), &node).is_none());
    }
}
