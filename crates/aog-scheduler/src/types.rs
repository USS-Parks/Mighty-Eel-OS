//! Scheduler domain types (Phase S). Every value here projects a real
//! `aog-estate` resource; nothing fabricates a signal it was not given. This is
//! the type-level half of the S1 defect purge — there is nowhere in these
//! structures to store an invented metric.

use aog_estate::{AttestationProfile, Capacity, Node, PlacementSpec, Workload, WorkloadKind};
use fabric_contracts::Classification;

/// A scheduler-facing projection of a [`Node`]'s real, reconciled state.
///
/// Built only from the node's `spec` and `status`. Absence is represented as
/// absence: a node with no `status` is `ready == false` with zero `allocatable`
/// and no `last_heartbeat` — honest zeros, not an invented headroom or an
/// assumed liveness (doctrine I-4).
#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    /// The node's resource name (its estate identity).
    pub name: String,
    /// Trust ring the node belongs to (1..=3).
    pub ring: u8,
    /// Highest classification the node is attested to hold — the S4 floor.
    pub attestation_floor: Classification,
    /// How the node proves that floor (platform, air-gap, PCR).
    pub attestation: AttestationProfile,
    /// Real liveness, reconciled from heartbeats by the node controller.
    /// `false` when the node has no status yet — fail-closed by construction.
    pub ready: bool,
    /// Real free capacity the node last reported. `Capacity::default()` (zero)
    /// when it has not reported — zero headroom, honestly, never invented.
    pub allocatable: Capacity,
    /// Whether the node has ever reported a heartbeat. `None` = never.
    pub last_heartbeat: Option<String>,
    /// Store revision this snapshot was taken at — provenance for the trace.
    pub resource_version: u64,
}

impl NodeSnapshot {
    /// Project a real [`Node`] into a snapshot. Reads `spec` + `status`
    /// verbatim; never synthesises a missing signal.
    pub fn from_node(node: &Node) -> Self {
        let status = node.status.as_ref();
        Self {
            name: node.metadata.name.clone(),
            ring: node.spec.ring,
            attestation_floor: node.spec.attestation_floor,
            attestation: node.spec.attestation.clone(),
            ready: status.is_some_and(|s| s.ready),
            allocatable: status.map_or_else(Capacity::default, |s| s.allocatable),
            last_heartbeat: status.and_then(|s| s.last_heartbeat.clone()),
            resource_version: node.metadata.resource_version,
        }
    }
}

/// A request to place one workload, projected from its real [`Workload`].
#[derive(Debug, Clone)]
pub struct ScheduleRequest {
    /// The workload's resource name.
    pub workload_name: String,
    /// What it runs as.
    pub workload_kind: WorkloadKind,
    /// The ring it must be placed within (S3).
    pub ring: u8,
    /// Its data-classification ceiling — must be `<=` the node's attestation
    /// floor to be placed (S4).
    pub classification_ceiling: Classification,
}

impl ScheduleRequest {
    /// Project a real [`Workload`] into a placement request.
    pub fn from_workload(workload: &Workload) -> Self {
        Self {
            workload_name: workload.metadata.name.clone(),
            workload_kind: workload.spec.workload_kind,
            ring: workload.spec.ring,
            classification_ceiling: workload.spec.classification_ceiling,
        }
    }
}

/// One filter's verdict for one node. Hard filters are deny-wins: a single
/// [`FilterVerdict::Unfit`] removes the node from candidacy (A1.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterVerdict {
    /// The node satisfies this filter.
    Fit,
    /// The node is rejected, carrying the filter that rejected it and why.
    Unfit {
        /// The rejecting filter's name.
        filter: &'static str,
        /// Human-readable reason, surfaced on a Pending workload.
        reason: String,
    },
}

impl FilterVerdict {
    /// Whether this verdict admits the node.
    pub fn is_fit(&self) -> bool {
        matches!(self, FilterVerdict::Fit)
    }

    /// Build an `Unfit` verdict.
    pub fn unfit(filter: &'static str, reason: impl Into<String>) -> Self {
        FilterVerdict::Unfit {
            filter,
            reason: reason.into(),
        }
    }
}

/// The real [`Node`] signals a decision consulted for one node — the audit
/// trail proving the decision traces to real inputs (the S1 gate). Every field
/// mirrors a concrete [`NodeSnapshot`] value; there is nowhere here to record a
/// fabricated number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalProvenance {
    /// The node these signals came from.
    pub node: String,
    /// Store revision the signals were read at.
    pub resource_version: u64,
    /// The node's reconciled readiness.
    pub ready: bool,
    /// Whether a real heartbeat was present.
    pub heartbeat_present: bool,
    /// The free capacity the node reported.
    pub allocatable: Capacity,
}

impl SignalProvenance {
    /// Capture the real signals of a snapshot.
    pub fn of(node: &NodeSnapshot) -> Self {
        Self {
            node: node.name.clone(),
            resource_version: node.resource_version,
            ready: node.ready,
            heartbeat_present: node.last_heartbeat.is_some(),
            allocatable: node.allocatable,
        }
    }
}

/// The full evaluation of one candidate node: the real signals consulted, each
/// filter verdict, and the composite score if the node survived.
#[derive(Debug, Clone)]
pub struct NodeEvaluation {
    /// The real signals this evaluation read.
    pub signals: SignalProvenance,
    /// Every filter's verdict, in registration order.
    pub verdicts: Vec<FilterVerdict>,
    /// `Some` iff the node passed every filter and every scorer produced a real
    /// score. `None` = filtered out or unscorable — never a fabricated fill-in.
    pub score: Option<f64>,
}

impl NodeEvaluation {
    /// Whether the node passed every hard filter.
    pub fn passed_filters(&self) -> bool {
        self.verdicts.iter().all(FilterVerdict::is_fit)
    }
}

/// Where the scheduler placed the workload — or why it could not.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleOutcome {
    /// Bound to `node` with the winning composite `score`.
    Scheduled {
        /// The chosen node's name.
        node: String,
        /// Its composite score (higher won).
        score: f64,
    },
    /// No node satisfied every hard filter (or all survivors were unscorable).
    /// The workload stays Pending — it is never force-placed on an unfit node to
    /// relieve pressure (A1.8, the S4 gate).
    Pending {
        /// Per-node rejection reasons, for the operator.
        reasons: Vec<String>,
    },
}

/// A complete, replayable scheduling decision: the outcome plus the per-node
/// evaluations that produced it. The evaluations are the decision's provenance.
#[derive(Debug, Clone)]
pub struct SchedulingDecision {
    /// The workload this decision is for.
    pub workload: String,
    /// The chosen binding, or Pending with reasons.
    pub outcome: ScheduleOutcome,
    /// Every candidate node's evaluation.
    pub evaluated: Vec<NodeEvaluation>,
}

impl SchedulingDecision {
    /// The node the workload was bound to, if any.
    pub fn scheduled_node(&self) -> Option<&str> {
        match &self.outcome {
            ScheduleOutcome::Scheduled { node, .. } => Some(node.as_str()),
            ScheduleOutcome::Pending { .. } => None,
        }
    }

    /// Whether the workload stayed Pending.
    pub fn is_pending(&self) -> bool {
        matches!(self.outcome, ScheduleOutcome::Pending { .. })
    }

    /// Render the winning binding as an estate [`PlacementSpec`]. The runtime
    /// token is minted at binding time (S7), so `token_id` is empty here.
    pub fn to_placement_spec(&self) -> Option<PlacementSpec> {
        self.scheduled_node().map(|node| PlacementSpec {
            workload: self.workload.clone(),
            node: node.to_owned(),
            token_id: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{Node, NodeSpec, NodeStatus};

    fn silent_node() -> Node {
        Node::new(
            "node-x",
            NodeSpec {
                ring: 3,
                attestation_floor: Classification::Secret,
                attestation: AttestationProfile::default(),
                capacity: Capacity::default(),
            },
        )
    }

    #[test]
    fn absent_status_projects_fail_closed() {
        let snap = NodeSnapshot::from_node(&silent_node());
        assert!(
            !snap.ready,
            "a node with no status must not project as ready"
        );
        assert_eq!(
            snap.allocatable,
            Capacity::default(),
            "no invented capacity"
        );
        assert!(snap.last_heartbeat.is_none());
    }

    #[test]
    fn present_status_projects_verbatim() {
        let mut node = silent_node();
        node.metadata.resource_version = 7;
        node.status = Some(NodeStatus {
            ready: true,
            last_heartbeat: Some("2026-07-04T12:00:00Z".to_owned()),
            allocatable: Capacity {
                cpu_millis: 4000,
                memory_mb: 8192,
                gpu: 1,
                max_workloads: 4,
            },
            ..NodeStatus::default()
        });

        let snap = NodeSnapshot::from_node(&node);
        assert!(snap.ready);
        assert_eq!(snap.resource_version, 7);
        assert_eq!(snap.allocatable.gpu, 1);

        let prov = SignalProvenance::of(&snap);
        assert!(prov.ready && prov.heartbeat_present);
        assert_eq!(prov.resource_version, 7);
        assert_eq!(prov.allocatable.gpu, 1);
    }
}
