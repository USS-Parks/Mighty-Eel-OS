//! Filter plugins. S1 ships the readiness foundation — the concrete deletion of
//! `mai-scheduler`'s fake-metrics defect (see the crate docs). Later prompts add
//! the ring filter (S3) and the attestation predicate (S4) here.

use crate::framework::Filter;
use crate::types::{FilterVerdict, NodeSnapshot, ScheduleRequest};

/// Hard filter: a node is a candidate only when it has actually reported —
/// `status.ready` is true and a heartbeat is present.
///
/// This inverts the defect the revival deletes. `mai-scheduler` scored a
/// zero-telemetry instance as maximally healthy; here a node with no reconciled
/// liveness is `Unfit`, never assumed live (doctrine I-4). Because a
/// [`NodeSnapshot`] projects a status-less node to `ready == false`, an
/// unmeasured node fails this filter by construction — there is no path by which
/// the absence of a signal becomes a favourable one.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadinessFilter;

impl Filter for ReadinessFilter {
    fn name(&self) -> &'static str {
        "readiness"
    }

    fn filter(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        match (node.ready, node.last_heartbeat.is_some()) {
            (true, true) => FilterVerdict::Fit,
            (false, _) => FilterVerdict::unfit(
                "readiness",
                "node status.ready is false (no reconciled liveness)",
            ),
            (true, false) => {
                FilterVerdict::unfit("readiness", "node has never reported a heartbeat")
            }
        }
    }
}

/// Hard filter: a node with a declared workload-slot capacity but none free is
/// saturated and rejected (S2). Free slots come from the node's reconciled
/// `allocatable` — real reported headroom — so a saturated node drops out of
/// candidacy rather than being packed further. A node that declares no slot
/// budget is not constrained on slots here (the utilisation scorer still weighs
/// its cpu/memory/gpu load).
#[derive(Debug, Clone, Copy, Default)]
pub struct CapacityFilter;

impl Filter for CapacityFilter {
    fn name(&self) -> &'static str {
        "capacity"
    }

    fn filter(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        if node.capacity.max_workloads > 0 && node.allocatable.max_workloads == 0 {
            return FilterVerdict::unfit(
                "capacity",
                "node is at workload capacity (0 free of declared slots)",
            );
        }
        FilterVerdict::Fit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{AttestationProfile, Capacity, WorkloadKind};
    use fabric_contracts::Classification;

    fn snap(ready: bool, heartbeat: bool) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready,
            capacity: Capacity::default(),
            allocatable: Capacity::default(),
            last_heartbeat: heartbeat.then(|| "t".to_owned()),
            resource_version: 1,
        }
    }

    fn cap_snap(total_slots: u32, free_slots: u32) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready: true,
            capacity: Capacity {
                max_workloads: total_slots,
                ..Capacity::default()
            },
            allocatable: Capacity {
                max_workloads: free_slots,
                ..Capacity::default()
            },
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
    fn ready_with_heartbeat_is_fit() {
        assert!(ReadinessFilter.filter(&req(), &snap(true, true)).is_fit());
    }

    #[test]
    fn not_ready_is_unfit() {
        assert!(!ReadinessFilter.filter(&req(), &snap(false, true)).is_fit());
    }

    #[test]
    fn ready_without_heartbeat_is_unfit() {
        // The defect inversion: a `ready` flag with no heartbeat is still unfit.
        assert!(!ReadinessFilter.filter(&req(), &snap(true, false)).is_fit());
    }

    #[test]
    fn saturated_node_is_unfit() {
        assert!(!CapacityFilter.filter(&req(), &cap_snap(8, 0)).is_fit());
    }

    #[test]
    fn node_with_free_slots_is_fit() {
        assert!(CapacityFilter.filter(&req(), &cap_snap(8, 3)).is_fit());
    }

    #[test]
    fn undeclared_slot_capacity_is_not_filtered() {
        // A node that declares no slot budget is not rejected on slots.
        assert!(CapacityFilter.filter(&req(), &cap_snap(0, 0)).is_fit());
    }
}
