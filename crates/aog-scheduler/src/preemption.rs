//! Preemption + priority (S8). When a workload cannot be placed because every
//! otherwise-fit node is saturated, a higher-priority workload may reclaim room
//! by evicting strictly-lower-priority, disruptible workloads — never violating
//! a hard predicate, never evicting a protected or equal/higher-priority victim.
//!
//! This module *plans* the eviction; executing it (draining the victims, then
//! binding) is the controller's job and ties the Phase-O disruption budgets
//! (O7). A workload with no lawful preemption stays Pending — pressure never
//! forces a placement.

use crate::types::{FilterVerdict, NodeEvaluation, SchedulingDecision};

/// A workload currently occupying a node, and whether it may be disrupted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Victim {
    /// The occupying workload's resource name.
    pub workload: String,
    /// Its scheduling priority. Only strictly-lower-priority victims may be
    /// preempted.
    pub priority: i32,
    /// Whether evicting it respects its disruption budget (the PDB-analog). The
    /// controller sets this `false` when an eviction would breach the budget;
    /// the planner then never selects it (ties O7).
    pub disruptible: bool,
}

/// The workloads a node currently hosts — the occupancy the planner reclaims
/// from. The controller builds this from the estate's `Placement`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOccupancy {
    /// The node's resource name (matches a `NodeSnapshot.name`).
    pub node: String,
    /// Its current occupants.
    pub victims: Vec<Victim>,
}

/// A lawful preemption: evict `victims` from `node` to free room for the
/// incoming workload. Evicting one workload frees one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreemptionPlan {
    /// The node to preempt on (was otherwise fit, blocked only on capacity).
    pub node: String,
    /// The workloads to evict.
    pub victims: Vec<String>,
}

/// Plan a preemption for a workload a prior [`SchedulingDecision`] left Pending.
///
/// A node is a preemption candidate only when it failed **exactly** the capacity
/// filter and passed every other hard filter — so freeing a slot makes it fit,
/// and a ring- or attestation-mismatched node is never a target (no hard
/// predicate is violated by preemption). On such a node the planner takes the
/// single lowest-priority victim that is both strictly lower priority than the
/// incoming workload and disruptible; evicting it frees one slot. Across
/// candidate nodes it picks the plan with the lowest-priority victim, ties
/// broken by node name. Returns `None` — the workload stays Pending — when no
/// lawful eviction exists.
pub fn plan_preemption(
    incoming_priority: i32,
    decision: &SchedulingDecision,
    occupancy: &[NodeOccupancy],
) -> Option<PreemptionPlan> {
    decision
        .evaluated
        .iter()
        .filter(|eval| only_capacity_blocked(eval))
        .filter_map(|eval| {
            let node = &eval.signals.node;
            let occ = occupancy.iter().find(|o| &o.node == node)?;
            let victim = occ
                .victims
                .iter()
                .filter(|v| v.disruptible && v.priority < incoming_priority)
                .min_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then_with(|| a.workload.cmp(&b.workload))
                })?;
            Some((victim.priority, node.clone(), victim.workload.clone()))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, node, victim)| PreemptionPlan {
            node,
            victims: vec![victim],
        })
}

/// True iff the node failed exactly the capacity filter and no other.
fn only_capacity_blocked(eval: &NodeEvaluation) -> bool {
    let mut blocked_on_capacity = false;
    for verdict in &eval.verdicts {
        match verdict {
            FilterVerdict::Fit => {}
            FilterVerdict::Unfit { filter, .. } if *filter == "capacity" => {
                blocked_on_capacity = true;
            }
            FilterVerdict::Unfit { .. } => return false,
        }
    }
    blocked_on_capacity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ScheduleOutcome, SignalProvenance};
    use aog_estate::Capacity;

    fn eval(node: &str, verdicts: Vec<FilterVerdict>) -> NodeEvaluation {
        NodeEvaluation {
            signals: SignalProvenance {
                node: node.to_owned(),
                resource_version: 1,
                ready: true,
                heartbeat_present: true,
                allocatable: Capacity::default(),
            },
            verdicts,
            score: None,
        }
    }

    fn pending(evals: Vec<NodeEvaluation>) -> SchedulingDecision {
        SchedulingDecision {
            workload: "incoming".to_owned(),
            outcome: ScheduleOutcome::Pending {
                reasons: Vec::new(),
            },
            evaluated: evals,
        }
    }

    fn cap_unfit() -> FilterVerdict {
        FilterVerdict::unfit("capacity", "saturated")
    }

    fn victim(name: &str, priority: i32, disruptible: bool) -> Victim {
        Victim {
            workload: name.to_owned(),
            priority,
            disruptible,
        }
    }

    fn occ(node: &str, victims: Vec<Victim>) -> NodeOccupancy {
        NodeOccupancy {
            node: node.to_owned(),
            victims,
        }
    }

    #[test]
    fn evicts_lower_priority_disruptible_victim() {
        let decision = pending(vec![eval("node-a", vec![FilterVerdict::Fit, cap_unfit()])]);
        let occupancy = vec![occ("node-a", vec![victim("low", 1, true)])];
        let plan = plan_preemption(5, &decision, &occupancy).expect("a plan");
        assert_eq!(plan.node, "node-a");
        assert_eq!(plan.victims, vec!["low".to_owned()]);
    }

    #[test]
    fn does_not_evict_equal_or_higher_priority() {
        let decision = pending(vec![eval("node-a", vec![cap_unfit()])]);
        let occupancy = vec![occ(
            "node-a",
            vec![victim("peer", 5, true), victim("boss", 9, true)],
        )];
        assert!(plan_preemption(5, &decision, &occupancy).is_none());
    }

    #[test]
    fn respects_disruption_budget() {
        // The only lower-priority victim is protected (disruptible == false).
        let decision = pending(vec![eval("node-a", vec![cap_unfit()])]);
        let occupancy = vec![occ("node-a", vec![victim("protected", 1, false)])];
        assert!(plan_preemption(5, &decision, &occupancy).is_none());
    }

    #[test]
    fn never_targets_a_ring_mismatched_node() {
        // node-a failed ring (not only capacity) → not a preemption target, even
        // though it hosts an evictable victim. No hard predicate is violated.
        let decision = pending(vec![eval(
            "node-a",
            vec![FilterVerdict::unfit("ring", "mismatch"), cap_unfit()],
        )]);
        let occupancy = vec![occ("node-a", vec![victim("low", 1, true)])];
        assert!(plan_preemption(5, &decision, &occupancy).is_none());
    }

    #[test]
    fn picks_lowest_priority_victim_across_nodes() {
        let decision = pending(vec![
            eval("node-a", vec![FilterVerdict::Fit, cap_unfit()]),
            eval("node-b", vec![FilterVerdict::Fit, cap_unfit()]),
        ]);
        let occupancy = vec![
            occ("node-a", vec![victim("mid", 3, true)]),
            occ("node-b", vec![victim("low", 1, true)]),
        ];
        let plan = plan_preemption(5, &decision, &occupancy).expect("a plan");
        assert_eq!(plan.node, "node-b");
        assert_eq!(plan.victims, vec!["low".to_owned()]);
    }
}
