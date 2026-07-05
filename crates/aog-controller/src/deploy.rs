//! O1 — the replica-set planner: the pure decision that turns a `Workload`'s
//! declared `replicas` into the set of `Placement`s that should exist, and the
//! excess that should be removed. It is the Deployment analog's core — "N
//! declared → N placed" — factored out of the binding controller (S7) so the
//! convergence logic is deterministic and unit-testable without OpenBao.
//!
//! Placements are **replica-indexed**: replica `i` of workload `w` is the
//! placement named `w-r<i>`. Indexing by ordinal (rather than by node, as the
//! M3b kernel did) is what lets a node host more than one replica of one
//! workload — packing — and what makes scale-down a precise "drop the ordinals
//! at or beyond desired".
//!
//! The planner runs the attested scheduler (Phase S) once per unfilled ordinal,
//! threading two pieces of per-pass state the scheduler cannot see on its own:
//!   * **spread** — every node already carrying a replica this pass is fed back
//!     as `already_placed_on`, so the S6 spread scorer fills fresh nodes first
//!     and only then packs;
//!   * **capacity** — a node's free workload slots are decremented locally per
//!     placement, so within one pass the planner never packs a node past the
//!     headroom it actually reported (the S2 `CapacityFilter` enforces the
//!     decremented budget). A node that declares no slot budget is unbounded.
//!
//! Attestation, ring, and readiness stay the scheduler's hard filters (S3/S4):
//! an ordinal the scheduler cannot satisfy is left short — never force-placed —
//! and the caller requeues it, exactly as a single replica would be (A1.8).

use std::collections::HashMap;

use aog_scheduler::{NodeSnapshot, ScheduleRequest, Scheduler};

/// The replica-indexed placement name for replica `ordinal` of `workload`.
#[must_use]
pub fn placement_name(workload: &str, ordinal: usize) -> String {
    format!("{workload}-r{ordinal}")
}

/// Parse the replica ordinal back out of a placement name, given its workload.
/// Returns `None` for a name that is not this workload's `-r<ordinal>` form.
#[must_use]
pub fn replica_index(workload: &str, placement: &str) -> Option<usize> {
    placement
        .strip_prefix(workload)?
        .strip_prefix("-r")?
        .parse()
        .ok()
}

/// The plan for one reconcile pass: which ordinals to create (and where), which
/// to delete, and how many desired ordinals could not be placed this pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplicaPlan {
    /// `(ordinal, node)` bindings to create — a new `Placement` each.
    pub create: Vec<(usize, String)>,
    /// Ordinals whose placement is excess and should be removed (scale-down:
    /// every ordinal at or beyond desired).
    pub delete: Vec<usize>,
    /// Desired ordinals left unplaced (no attestation-satisfying node with room).
    /// `> 0` means the workload stays short and the caller requeues.
    pub short: usize,
}

impl ReplicaPlan {
    /// Whether the estate already matches desired — nothing to create or delete,
    /// nothing short.
    #[must_use]
    pub fn is_converged(&self) -> bool {
        self.create.is_empty() && self.delete.is_empty() && self.short == 0
    }

    /// Whether this pass touches the estate (has creates or deletes). A pass with
    /// only `short > 0` changes nothing but must requeue.
    #[must_use]
    pub fn mutates(&self) -> bool {
        !self.create.is_empty() || !self.delete.is_empty()
    }
}

/// Free workload-slot budget for a node this pass: `Some(n)` bounded, `None`
/// unbounded (the node declared no slot capacity, so the `CapacityFilter` does
/// not constrain it on slots).
fn seed_free(snapshots: &[NodeSnapshot]) -> HashMap<String, Option<u32>> {
    snapshots
        .iter()
        .map(|s| {
            let budget = (s.capacity.max_workloads > 0).then_some(s.allocatable.max_workloads);
            (s.name.clone(), budget)
        })
        .collect()
}

/// Plan the replica set. `existing` maps each already-placed ordinal to its
/// node (parsed from live `Placement`s). `request` is the workload's schedule
/// template (ring, ceiling, kind); its `already_placed_on` is ignored — the
/// planner manages spread itself.
#[must_use]
pub fn plan_replicas(
    desired: usize,
    existing: &std::collections::BTreeMap<usize, String>,
    scheduler: &Scheduler,
    request: &ScheduleRequest,
    snapshots: &[NodeSnapshot],
) -> ReplicaPlan {
    let mut plan = ReplicaPlan::default();

    // Scale down: every ordinal at or beyond desired is excess.
    for &ordinal in existing.keys() {
        if ordinal >= desired {
            plan.delete.push(ordinal);
        }
    }

    // Per-pass capacity budget and the spread multiset, seeded from survivors.
    // A survivor's slot is already reflected in the node's reported allocatable,
    // so only *this pass's* new placements decrement the local budget.
    let mut free = seed_free(snapshots);
    let mut used: Vec<String> = existing
        .range(..desired)
        .map(|(_, node)| node.clone())
        .collect();

    // Scale up: fill each unfilled ordinal in [0, desired).
    for ordinal in 0..desired {
        if existing.contains_key(&ordinal) {
            continue;
        }
        // Candidate snapshots reflecting the remaining local slot budget, so the
        // CapacityFilter excludes a node already packed full this pass.
        let candidates: Vec<NodeSnapshot> = snapshots
            .iter()
            .map(|s| {
                let mut s = s.clone();
                if let Some(Some(rem)) = free.get(&s.name) {
                    s.allocatable.max_workloads = *rem;
                }
                s
            })
            .collect();
        let mut req = request.clone();
        req.already_placed_on.clone_from(&used);
        match scheduler.schedule(&req, &candidates).scheduled_node() {
            Some(node) => {
                let node = node.to_owned();
                if let Some(Some(rem)) = free.get_mut(&node) {
                    *rem = rem.saturating_sub(1);
                }
                used.push(node.clone());
                plan.create.push((ordinal, node));
            }
            None => plan.short += 1,
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use aog_estate::{AttestationProfile, Capacity, WorkloadKind};
    use aog_scheduler::attested_scheduler;
    use fabric_contracts::Classification;

    /// A ready, public-classification node with `slots` declared workload slots
    /// and `free` of them available. Public ceiling needs no hardware root, so
    /// the attestation filter passes — keeps these tests about count/packing.
    fn node(name: &str, slots: u32, free: u32) -> NodeSnapshot {
        NodeSnapshot {
            name: name.to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready: true,
            capacity: Capacity {
                max_workloads: slots,
                ..Capacity::default()
            },
            allocatable: Capacity {
                max_workloads: free,
                ..Capacity::default()
            },
            last_heartbeat: Some("t".to_owned()),
            resource_version: 1,
        }
    }

    fn request() -> ScheduleRequest {
        ScheduleRequest {
            workload_name: "gw".to_owned(),
            workload_kind: WorkloadKind::Gateway,
            ring: 1,
            classification_ceiling: Classification::Public,
            already_placed_on: Vec::new(),
        }
    }

    fn plan(desired: usize, existing: &[(usize, &str)], nodes: &[NodeSnapshot]) -> ReplicaPlan {
        let existing: BTreeMap<usize, String> = existing
            .iter()
            .map(|(i, n)| (*i, (*n).to_owned()))
            .collect();
        plan_replicas(desired, &existing, &attested_scheduler(), &request(), nodes)
    }

    #[test]
    fn name_round_trips_through_index() {
        assert_eq!(placement_name("gw", 3), "gw-r3");
        assert_eq!(replica_index("gw", "gw-r3"), Some(3));
        assert_eq!(replica_index("gw", "gw-node-a"), None);
        // A workload whose own name contains "-r" is parsed from its full prefix.
        assert_eq!(replica_index("a-r-b", "a-r-b-r7"), Some(7));
    }

    #[test]
    fn scale_up_spreads_across_distinct_nodes() {
        let nodes = vec![node("node-a", 4, 4), node("node-b", 4, 4)];
        let p = plan(2, &[], &nodes);
        assert_eq!(p.short, 0);
        assert!(p.delete.is_empty());
        assert_eq!(p.create.len(), 2);
        let placed: Vec<&str> = p.create.iter().map(|(_, n)| n.as_str()).collect();
        assert!(
            placed.contains(&"node-a") && placed.contains(&"node-b"),
            "spread across both"
        );
    }

    #[test]
    fn packs_when_replicas_exceed_nodes() {
        // 3 replicas, 2 nodes with room for 2 each: all placed, a node repeats.
        let nodes = vec![node("node-a", 2, 2), node("node-b", 2, 2)];
        let p = plan(3, &[], &nodes);
        assert_eq!(p.short, 0);
        assert_eq!(p.create.len(), 3);
        let mut used: Vec<&str> = p.create.iter().map(|(_, n)| n.as_str()).collect();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), 2, "packed onto both nodes");
        assert!(
            p.create.len() > used.len(),
            "packing: more replicas than distinct nodes"
        );
    }

    #[test]
    fn capacity_bounds_packing_and_leaves_the_rest_short() {
        // 3 replicas, 2 nodes with room for exactly 1 each → 2 placed, 1 short.
        let nodes = vec![node("node-a", 1, 1), node("node-b", 1, 1)];
        let p = plan(3, &[], &nodes);
        assert_eq!(p.create.len(), 2);
        assert_eq!(p.short, 1, "the third replica has nowhere to go");
    }

    #[test]
    fn undeclared_slot_capacity_packs_freely() {
        // A node declaring no slot budget is unbounded: all 3 land on it.
        let nodes = vec![node("solo", 0, 0)];
        let p = plan(3, &[], &nodes);
        assert_eq!(p.short, 0);
        assert_eq!(p.create.len(), 3);
        assert!(p.create.iter().all(|(_, n)| n == "solo"));
    }

    #[test]
    fn scale_down_drops_the_highest_ordinals() {
        let nodes = vec![node("node-a", 4, 4), node("node-b", 4, 4)];
        let p = plan(1, &[(0, "node-a"), (1, "node-b"), (2, "node-a")], &nodes);
        assert!(p.create.is_empty(), "ordinal 0 already exists");
        let mut deleted = p.delete.clone();
        deleted.sort_unstable();
        assert_eq!(deleted, vec![1, 2], "ordinals >= desired are excess");
        assert_eq!(p.short, 0);
    }

    #[test]
    fn already_converged_is_a_no_op() {
        let nodes = vec![node("node-a", 4, 4), node("node-b", 4, 4)];
        let p = plan(2, &[(0, "node-a"), (1, "node-b")], &nodes);
        assert!(p.is_converged());
        assert!(!p.mutates());
    }

    #[test]
    fn no_ready_node_leaves_every_replica_short() {
        let mut down = node("node-a", 4, 4);
        down.ready = false;
        down.last_heartbeat = None;
        let p = plan(2, &[], &[down]);
        assert!(p.create.is_empty());
        assert_eq!(p.short, 2, "an unready node hosts nothing (fail-closed)");
    }
}
