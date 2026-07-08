//! O7 — disruption budgets + node maintenance: cordon a node out of scheduling,
//! then drain its workloads off it **within a disruption budget**, so a node can
//! be serviced without ever taking too many replicas of one workload down at
//! once, and without a workload ever crossing its ring.
//!
//! Cordon is a label ([`CORDON_LABEL`]) on the `Node` — no schema change. The
//! scheduler excludes a cordoned node from candidacy (`node_snapshots` skips
//! [`is_cordoned`] nodes), so it takes no new placements and a drained replica is
//! never re-placed back onto it. The [`MaintenanceController`] drains a cordoned
//! node's `Placement`s in bounded batches ([`plan_drain`]): at most
//! `disruption_budget` replicas of any one workload are evicted per pass, and the
//! scheduler re-places them on other **same-ring** nodes (the S3 ring filter is
//! unchanged), so ring isolation holds throughout the drain.
//!
//! The drain plan is a **pure function** — deterministic, no clock — so
//! "within budget" is provable. Token cleanup on eviction (revoking the drained
//! replica's runtime token) rides an optional OpenBao seam, mirroring O1
//! scale-down; without it the controller drains estate-only (tests).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use wsf_bridge::OpenBaoAuth;

use aog_estate::{Kind, ObjectMeta, ResourceObject};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// The label that cordons a node: `Node.metadata.labels[CORDON_LABEL] == "true"`
/// takes it out of scheduling and marks it for maintenance drain.
pub const CORDON_LABEL: &str = "loom.io/unschedulable";

/// Cadence between drain passes — long enough that the scheduler re-places an
/// evicted batch on another node before the next batch is taken down.
const REQUEUE: Duration = Duration::from_secs(5);

/// Whether `meta` marks a cordoned (unschedulable / draining) object.
#[must_use]
pub fn is_cordoned(meta: &ObjectMeta) -> bool {
    meta.labels.get(CORDON_LABEL).map(String::as_str) == Some("true")
}

/// One placement eligible to be drained off a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainCandidate {
    /// The placement's resource name.
    pub placement: String,
    /// The workload it hosts (the disruption budget is per workload).
    pub workload: String,
}

/// The drain decision for one pass: which placements to evict now and which to
/// defer to a later pass (so the budget is never exceeded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainPlan {
    /// Placements to evict this pass.
    pub evict: Vec<String>,
    /// Placements held back — draining them now would breach the budget.
    pub defer: Vec<String>,
}

impl DrainPlan {
    /// Whether the node still has placements to drain in a later pass.
    #[must_use]
    pub fn has_more(&self) -> bool {
        !self.defer.is_empty()
    }
}

/// Plan one drain pass (the O7 gate: deterministic, budget-respecting). At most
/// `disruption_budget` replicas of any single workload are evicted this pass; the
/// rest are deferred. A budget of 0 is treated as 1 so a drain always progresses.
/// Candidates are processed in the given order — the same node always drains the
/// same way.
#[must_use]
pub fn plan_drain(candidates: &[DrainCandidate], disruption_budget: u32) -> DrainPlan {
    let budget = disruption_budget.max(1);
    let mut evicted: BTreeMap<&str, u32> = BTreeMap::new();
    let mut plan = DrainPlan::default();
    for c in candidates {
        let count = evicted.entry(c.workload.as_str()).or_default();
        if *count < budget {
            *count += 1;
            plan.evict.push(c.placement.clone());
        } else {
            plan.defer.push(c.placement.clone());
        }
    }
    plan
}

/// Drains cordoned nodes within a disruption budget. Run it on a `"Node/"`
/// informer with a resync heartbeat (so a node cordoned without another Node edit
/// is still picked up).
#[derive(Clone)]
pub struct MaintenanceController {
    client: EstateClient,
    disruption_budget: u32,
    revoke: Option<(Arc<OpenBaoAuth>, String)>,
}

impl MaintenanceController {
    /// A controller that evicts at most `disruption_budget` replicas of a workload
    /// per drain pass. Estate-only (no runtime-token cleanup) until
    /// [`with_token_revocation`](Self::with_token_revocation) is set.
    #[must_use]
    pub fn new(client: EstateClient, disruption_budget: u32) -> Self {
        Self {
            client,
            disruption_budget,
            revoke: None,
        }
    }

    /// Also revoke each drained replica's runtime token (`delete_kv` at
    /// `<token_prefix>/<placement>`), the same cleanup O1 scale-down does.
    #[must_use]
    pub fn with_token_revocation(
        mut self,
        openbao: Arc<OpenBaoAuth>,
        token_prefix: impl Into<String>,
    ) -> Self {
        self.revoke = Some((openbao, token_prefix.into()));
        self
    }

    /// The placements currently hosted on `node`, as drain candidates.
    async fn candidates_on(&self, node: &str) -> Result<Vec<DrainCandidate>, ReconcileError> {
        let mut candidates = Vec::new();
        for object in self.client.list(Kind::Placement).await? {
            if let ResourceObject::Placement(placement) = object
                && placement.spec.node == node
            {
                candidates.push(DrainCandidate {
                    placement: placement.metadata.name,
                    workload: placement.spec.workload,
                });
            }
        }
        // Stable order → deterministic drain.
        candidates.sort_by(|a, b| a.placement.cmp(&b.placement));
        Ok(candidates)
    }

    /// Evict one placement: revoke its token (when configured) and delete it. The
    /// scheduler re-places the replica on another same-ring node.
    async fn evict(&self, placement: &str) -> Result<(), ReconcileError> {
        if let Some((openbao, prefix)) = &self.revoke {
            let vault = openbao
                .login()
                .await
                .map_err(|e| ReconcileError(e.to_string()))?;
            // Best-effort: an already-absent token is convergence, not error.
            let _ = openbao
                .delete_kv(&vault, &format!("{prefix}/{placement}"))
                .await;
        }
        self.client.delete(Kind::Placement, placement).await
    }

    async fn reconcile_node(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Node(node)) = self.client.get(Kind::Node, name).await? else {
            return Ok(Action::Done);
        };
        // Only a cordoned node drains; an uncordoned node is left alone.
        if !is_cordoned(&node.metadata) {
            return Ok(Action::Done);
        }

        let candidates = self.candidates_on(name).await?;
        if candidates.is_empty() {
            return Ok(Action::Done); // fully drained
        }
        let plan = plan_drain(&candidates, self.disruption_budget);
        for placement in &plan.evict {
            self.evict(placement).await?;
        }
        // Deferred placements drain on later passes, once the scheduler has
        // restored availability by re-placing this batch elsewhere.
        if plan.has_more() {
            Ok(Action::RequeueAfter(REQUEUE))
        } else {
            Ok(Action::Done)
        }
    }
}

impl Reconciler for MaintenanceController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::Node, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_node(name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{Node, NodeSpec};
    use fabric_contracts::Classification;

    fn candidate(placement: &str, workload: &str) -> DrainCandidate {
        DrainCandidate {
            placement: placement.to_owned(),
            workload: workload.to_owned(),
        }
    }

    #[test]
    fn is_cordoned_reads_the_label() {
        let mut node = Node::new(
            "n1",
            NodeSpec {
                ring: 1,
                attestation_floor: Classification::Public,
                attestation: aog_estate::AttestationProfile::default(),
                capacity: aog_estate::Capacity::default(),
            },
        );
        assert!(!is_cordoned(&node.metadata));
        node.metadata
            .labels
            .insert(CORDON_LABEL.to_owned(), "true".to_owned());
        assert!(is_cordoned(&node.metadata));
        node.metadata
            .labels
            .insert(CORDON_LABEL.to_owned(), "false".to_owned());
        assert!(!is_cordoned(&node.metadata), "only \"true\" cordons");
    }

    #[test]
    fn a_drain_evicts_at_most_the_budget_per_workload() {
        // Workload gw has 4 replicas on the node; budget 2 → 2 evicted, 2 deferred.
        let candidates = vec![
            candidate("gw-r0", "gw"),
            candidate("gw-r1", "gw"),
            candidate("gw-r2", "gw"),
            candidate("gw-r3", "gw"),
        ];
        let plan = plan_drain(&candidates, 2);
        assert_eq!(
            plan.evict.len(),
            2,
            "no more than the budget comes down at once"
        );
        assert_eq!(plan.defer.len(), 2);
        assert!(plan.has_more());
    }

    #[test]
    fn the_budget_is_per_workload_not_per_node() {
        // Two workloads, budget 1 each → one of each evicted this pass.
        let candidates = vec![
            candidate("gw-r0", "gw"),
            candidate("gw-r1", "gw"),
            candidate("api-r0", "api"),
            candidate("api-r1", "api"),
        ];
        let plan = plan_drain(&candidates, 1);
        assert_eq!(
            plan.evict.len(),
            2,
            "one replica of each of the two workloads"
        );
        assert!(plan.evict.contains(&"gw-r0".to_owned()));
        assert!(plan.evict.contains(&"api-r0".to_owned()));
        assert_eq!(plan.defer.len(), 2);
    }

    #[test]
    fn a_zero_budget_still_progresses() {
        let candidates = vec![candidate("gw-r0", "gw"), candidate("gw-r1", "gw")];
        let plan = plan_drain(&candidates, 0);
        assert_eq!(plan.evict.len(), 1, "a zero budget is treated as one");
    }

    #[test]
    fn draining_is_deterministic() {
        let candidates = vec![candidate("gw-r0", "gw"), candidate("gw-r1", "gw")];
        assert_eq!(plan_drain(&candidates, 1), plan_drain(&candidates, 1));
    }

    #[test]
    fn an_empty_node_has_nothing_to_drain() {
        assert_eq!(plan_drain(&[], 2), DrainPlan::default());
    }
}
