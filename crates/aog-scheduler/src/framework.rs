//! The filter/score plugin framework (Phase S), revived from `mai-scheduler`'s
//! `Scheduler`/`PlacementEngine` shape and rebuilt for AOG workload placement.
//!
//! Two extension seams:
//! - [`Filter`] — a hard, deny-wins predicate. Any `Unfit` removes the node.
//!   The ring filter (S3) and the attestation predicate (S4) register here.
//! - [`Scorer`] — a soft preference over survivors, returning `Option<f64>`.
//!   `None` **abstains** (no real signal, so no preference) — it contributes
//!   nothing and is never replaced by a fabricated value (doctrine I-4). A
//!   scorer never *excludes* a node; that is a filter's job. The budget/ROI (S5)
//!   and spread/HA (S6) scorers register here.
//!
//! The engine is deterministic: no clock, no RNG. Ties in score break by node
//! name, so the same estate always yields the same decision (replayability).

use crate::types::{
    FilterVerdict, NodeEvaluation, NodeSnapshot, ScheduleOutcome, ScheduleRequest,
    SchedulingDecision, SignalProvenance,
};

/// A hard placement predicate. Deny-wins: one `Unfit` removes the node.
pub trait Filter: Send + Sync {
    /// Stable name, recorded in the decision trace and rejection reasons.
    fn name(&self) -> &'static str;

    /// Judge whether `node` may host `request`.
    fn filter(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict;
}

/// A soft preference over nodes that survived filtering.
pub trait Scorer: Send + Sync {
    /// Stable name for diagnostics.
    fn name(&self) -> &'static str;

    /// Score `node` for `request` — higher is more preferred. Returns `None` to
    /// **abstain** when there is no real signal to score on; the framework then
    /// contributes nothing for this scorer and never fabricates a value in its
    /// place (doctrine I-4). Abstaining expresses no preference — it does not
    /// exclude the node; hard exclusion is a [`Filter`]'s job.
    fn score(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> Option<f64>;
}

struct WeightedScorer {
    weight: f64,
    scorer: Box<dyn Scorer>,
}

/// The placement engine: a filter chain and a weighted scorer set. Build it
/// with [`Scheduler::with_filter`] / [`Scheduler::with_scorer`], then call
/// [`Scheduler::schedule`].
#[derive(Default)]
pub struct Scheduler {
    filters: Vec<Box<dyn Filter>>,
    scorers: Vec<WeightedScorer>,
}

impl Scheduler {
    /// An empty scheduler: no filters, no scorers. With no filters every node is
    /// a candidate; with no scorers every survivor ties at a neutral `0.0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hard filter.
    pub fn with_filter(mut self, filter: impl Filter + 'static) -> Self {
        self.filters.push(Box::new(filter));
        self
    }

    /// Register a weighted scorer.
    pub fn with_scorer(mut self, weight: f64, scorer: impl Scorer + 'static) -> Self {
        self.scorers.push(WeightedScorer {
            weight,
            scorer: Box::new(scorer),
        });
        self
    }

    /// The registered filter names, in order (diagnostics).
    pub fn filter_names(&self) -> Vec<&'static str> {
        self.filters.iter().map(|f| f.name()).collect()
    }

    /// Evaluate every node through the filter chain, score the survivors, and
    /// bind the workload to the highest scorer. A workload with no surviving node
    /// stays [`ScheduleOutcome::Pending`] — never force-placed.
    pub fn schedule(
        &self,
        request: &ScheduleRequest,
        nodes: &[NodeSnapshot],
    ) -> SchedulingDecision {
        let evaluated: Vec<NodeEvaluation> =
            nodes.iter().map(|n| self.evaluate(request, n)).collect();

        let winner = evaluated
            .iter()
            .filter_map(|e| e.score.map(|s| (s, e.signals.node.as_str())))
            .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.cmp(a.1)));

        let outcome = match winner {
            Some((score, node)) => ScheduleOutcome::Scheduled {
                node: node.to_owned(),
                score,
            },
            None => ScheduleOutcome::Pending {
                reasons: Self::pending_reasons(&evaluated, nodes.len()),
            },
        };

        SchedulingDecision {
            workload: request.workload_name.clone(),
            outcome,
            evaluated,
        }
    }

    fn evaluate(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> NodeEvaluation {
        let signals = SignalProvenance::of(node);
        let mut verdicts = Vec::with_capacity(self.filters.len());
        let mut fit = true;
        for filter in &self.filters {
            let verdict = filter.filter(request, node);
            fit &= verdict.is_fit();
            verdicts.push(verdict);
        }
        let score = fit.then(|| self.score_node(request, node));
        NodeEvaluation {
            signals,
            verdicts,
            score,
        }
    }

    /// Composite score = Σ weightᵢ · scorerᵢ(node) over the scorers that produced
    /// a real score. A scorer returning `None` **abstains** — it contributes
    /// nothing, and the engine never invents a value in its place (doctrine I-4).
    /// A filter-surviving node is always placeable, so this always yields a
    /// score; when no scorer scored it (or none are registered) the node ties at
    /// a neutral `0.0` — a fact (filters passed, no preference expressed), not a
    /// fabricated signal. Hard exclusion is the filters' job, not a scorer's.
    fn score_node(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> f64 {
        let mut total = 0.0;
        for ws in &self.scorers {
            if let Some(component) = ws.scorer.score(request, node) {
                total += ws.weight * component;
            }
        }
        total
    }

    fn pending_reasons(evaluated: &[NodeEvaluation], node_count: usize) -> Vec<String> {
        if node_count == 0 {
            return vec!["no nodes in estate".to_owned()];
        }
        let mut reasons = Vec::new();
        for e in evaluated {
            for verdict in &e.verdicts {
                if let FilterVerdict::Unfit { filter, reason } = verdict {
                    reasons.push(format!("{}: {reason} [{filter}]", e.signals.node));
                }
            }
        }
        if reasons.is_empty() {
            reasons.push("no node satisfied all hard filters".to_owned());
        }
        reasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{AttestationProfile, Capacity, WorkloadKind};
    use fabric_contracts::Classification;

    fn snap(name: &str, ready: bool) -> NodeSnapshot {
        NodeSnapshot {
            name: name.to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready,
            capacity: Capacity::default(),
            allocatable: Capacity::default(),
            last_heartbeat: ready.then(|| "t".to_owned()),
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

    struct Reject;
    impl Filter for Reject {
        fn name(&self) -> &'static str {
            "reject"
        }
        fn filter(&self, _: &ScheduleRequest, _: &NodeSnapshot) -> FilterVerdict {
            FilterVerdict::unfit("reject", "always")
        }
    }

    /// Scores by the node's real reported GPU count — a real-signal scorer.
    struct GpuScore;
    impl Scorer for GpuScore {
        fn name(&self) -> &'static str {
            "gpu"
        }
        fn score(&self, _: &ScheduleRequest, node: &NodeSnapshot) -> Option<f64> {
            Some(f64::from(node.allocatable.gpu))
        }
    }

    /// Never has a signal to offer — always abstains.
    struct NoSignal;
    impl Scorer for NoSignal {
        fn name(&self) -> &'static str {
            "no-signal"
        }
        fn score(&self, _: &ScheduleRequest, _: &NodeSnapshot) -> Option<f64> {
            None
        }
    }

    #[test]
    fn no_scorers_ties_break_by_name() {
        let sched = Scheduler::new();
        let nodes = vec![snap("bravo", true), snap("alpha", true)];
        let d = sched.schedule(&req(), &nodes);
        assert_eq!(d.scheduled_node(), Some("alpha"));
    }

    #[test]
    fn reject_filter_yields_pending() {
        let sched = Scheduler::new().with_filter(Reject);
        assert_eq!(sched.filter_names(), vec!["reject"]);
        let d = sched.schedule(&req(), &[snap("alpha", true)]);
        assert!(d.is_pending());
        match d.outcome {
            ScheduleOutcome::Pending { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("reject")));
            }
            ScheduleOutcome::Scheduled { .. } => panic!("expected pending"),
        }
    }

    #[test]
    fn abstaining_scorer_still_places() {
        // A scorer with no signal abstains; with no filter rejecting it the node
        // is still placed at a neutral score. Abstention is not exclusion.
        let sched = Scheduler::new().with_scorer(1.0, NoSignal);
        let d = sched.schedule(&req(), &[snap("alpha", true)]);
        assert_eq!(d.scheduled_node(), Some("alpha"));
    }

    #[test]
    fn higher_real_score_wins() {
        let sched = Scheduler::new().with_scorer(1.0, GpuScore);
        let mut big = snap("alpha", true);
        big.allocatable.gpu = 4;
        let mut small = snap("bravo", true);
        small.allocatable.gpu = 1;
        let d = sched.schedule(&req(), &[small, big]);
        assert_eq!(d.scheduled_node(), Some("alpha"));
    }

    #[test]
    fn empty_estate_is_pending() {
        let d = Scheduler::new().schedule(&req(), &[]);
        assert!(d.is_pending());
        assert!(d.evaluated.is_empty());
    }
}
