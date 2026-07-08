//! O4 — the budget-/ROI-aware autoscaler: it scales a `Workload` on **load and
//! economics together**, not load alone. Saturated and affordable → scale up;
//! saturated but out of budget or at the replica ceiling → *recommend hardware*
//! rather than overspend; idle → consolidate down; budget-inefficient (spending
//! for too little return) → scale down even when not idle.
//!
//! The decision is a **pure function** ([`autoscale`]) of the current replica
//! count, a real signal snapshot ([`AutoscaleSignals`]) projected from node
//! utilization (aog-scheduler) and metered spend/ROI (the gateway meter /
//! SpendLedger), and a policy — no clock, no RNG. The same fixture always yields
//! the same decision (the O4 gate). Absence of telemetry is fail-closed: with no
//! signal the autoscaler holds rather than scaling on a guess (doctrine I-4).
//!
//! The controller applies scale up/down by writing `Workload.spec.replicas` (the
//! HPA pattern — O1's replica-set convergence then places/reclaims to match). A
//! `RecommendHardware` verdict is a capital decision for an operator, surfaced by
//! the pure decision (as the gateway ROI recommender already does for humans);
//! the controller never fabricates capacity it cannot afford.

use std::future::Future;
use std::sync::Arc;

use aog_estate::{Kind, ResourceObject};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// A real load + economics snapshot for one workload. Every field is a projection
/// of measured state; nothing here is invented (an unmeasured workload yields
/// `None` from the probe, not a favourable default).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoscaleSignals {
    /// Mean utilization across the workload's replicas, in `[0, 1]`.
    pub utilization: f64,
    /// Budget headroom fraction in `[0, 1]`: remaining / limit. `0` = exhausted.
    pub budget_headroom: f64,
    /// ROI efficiency in `[0, 1]`: value delivered per unit spend. Below the
    /// policy floor the replicas are burning budget for too little return.
    pub roi: f64,
}

/// The autoscale policy — watermarks and bounds. Cluster-wide in M3c (per-workload
/// policy is a later refinement); the defaults are conservative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoscalePolicy {
    pub min_replicas: u32,
    pub max_replicas: u32,
    /// Utilization at/above which the workload is saturated (scale up).
    pub high_watermark: f64,
    /// Utilization at/below which the workload is idle (consolidate down).
    pub low_watermark: f64,
    /// ROI at/below which spend is inefficient (scale down even if not idle).
    pub roi_floor: f64,
    /// Budget headroom below which a scale-up is unaffordable (recommend hardware).
    pub budget_floor: f64,
}

impl Default for AutoscalePolicy {
    fn default() -> Self {
        Self {
            min_replicas: 1,
            max_replicas: 10,
            high_watermark: 0.80,
            low_watermark: 0.20,
            roi_floor: 0.30,
            budget_floor: 0.10,
        }
    }
}

/// Why the autoscaler chose to scale down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleReason {
    /// Utilization below the low watermark — consolidate.
    Idle,
    /// ROI below the floor — spending for too little return.
    BudgetInefficient,
}

/// The autoscaler's deterministic verdict for one workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleDecision {
    /// Add a replica (to `to`), within budget and the replica ceiling.
    ScaleUp { to: u32 },
    /// Remove a replica (to `to`), for `reason`.
    ScaleDown { to: u32, reason: ScaleReason },
    /// Saturated but cannot scale (out of budget or at the ceiling): the load
    /// needs *more hardware*, not more replicas on the same tier.
    RecommendHardware,
    /// No change warranted.
    Hold,
}

/// The pure autoscale decision (the O4 gate: deterministic, budget-respecting).
///
/// Priority: saturation (a load need) is handled first — scale up when it is
/// both affordable and under the ceiling, else recommend hardware, never
/// overspend. When not saturated, economics decide: budget-inefficient spend
/// scales down, then idleness consolidates. A workload at `min_replicas` is never
/// scaled below it.
#[must_use]
pub fn autoscale(
    current: u32,
    signals: &AutoscaleSignals,
    policy: &AutoscalePolicy,
) -> ScaleDecision {
    let can_shrink = current > policy.min_replicas;

    // Saturation first: the workload needs capacity now.
    if signals.utilization >= policy.high_watermark {
        let affordable = signals.budget_headroom >= policy.budget_floor;
        let has_room = current < policy.max_replicas;
        return if affordable && has_room {
            ScaleDecision::ScaleUp {
                to: current.saturating_add(1),
            }
        } else {
            // Can't add a replica within budget/ceiling — this is a hardware call.
            ScaleDecision::RecommendHardware
        };
    }

    // Not saturated: economics govern. Budget-inefficiency scales down first.
    if signals.roi <= policy.roi_floor && can_shrink {
        return ScaleDecision::ScaleDown {
            to: current.saturating_sub(1),
            reason: ScaleReason::BudgetInefficient,
        };
    }
    // Then idleness consolidates.
    if signals.utilization <= policy.low_watermark && can_shrink {
        return ScaleDecision::ScaleDown {
            to: current.saturating_sub(1),
            reason: ScaleReason::Idle,
        };
    }
    ScaleDecision::Hold
}

/// Supplies real load/economics signals for a workload, from node utilization and
/// the meter. Synchronous — a telemetry snapshot the caller already holds. `None`
/// means "no signal yet": the autoscaler then holds (fail-closed).
pub trait AutoscaleProbe: Send + Sync {
    fn signals(&self, workload: &str) -> Option<AutoscaleSignals>;
}

/// Scales `Workload`s on load + budget/ROI. Run it on a `"Workload/"` informer
/// with a resync heartbeat (utilization changes without a spec edit).
#[derive(Clone)]
pub struct AutoscaleController<P: AutoscaleProbe> {
    client: EstateClient,
    policy: AutoscalePolicy,
    probe: Arc<P>,
}

impl<P: AutoscaleProbe> AutoscaleController<P> {
    #[must_use]
    pub fn new(client: EstateClient, policy: AutoscalePolicy, probe: Arc<P>) -> Self {
        Self {
            client,
            policy,
            probe,
        }
    }

    async fn reconcile_workload(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::Workload(workload)) =
            self.client.get(Kind::Workload, name).await?
        else {
            return Ok(Action::Done);
        };
        if workload.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done);
        }
        // No telemetry → hold (never scale on a guess).
        let Some(signals) = self.probe.signals(name) else {
            return Ok(Action::Done);
        };

        let target = match autoscale(workload.spec.replicas, &signals, &self.policy) {
            ScaleDecision::ScaleUp { to } | ScaleDecision::ScaleDown { to, .. } => to,
            // RecommendHardware / Hold: the replica count stands. The hardware
            // recommendation is an operator/console concern (the ROI recommender),
            // not an automatic capacity fabrication.
            ScaleDecision::RecommendHardware | ScaleDecision::Hold => return Ok(Action::Done),
        };
        if target != workload.spec.replicas {
            let mut converged = workload;
            converged.spec.replicas = target;
            self.client
                .update(ResourceObject::Workload(converged))
                .await?;
        }
        Ok(Action::Done)
    }
}

impl<P: AutoscaleProbe + 'static> Reconciler for AutoscaleController<P> {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let client = self.client.clone();
        let policy = self.policy;
        let probe = Arc::clone(&self.probe);
        let key = key.to_owned();
        async move {
            let Some((Kind::Workload, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            AutoscaleController {
                client,
                policy,
                probe,
            }
            .reconcile_workload(name)
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(utilization: f64, budget_headroom: f64, roi: f64) -> AutoscaleSignals {
        AutoscaleSignals {
            utilization,
            budget_headroom,
            roi,
        }
    }

    fn policy() -> AutoscalePolicy {
        AutoscalePolicy::default()
    }

    #[test]
    fn saturated_and_affordable_scales_up() {
        let d = autoscale(3, &signals(0.9, 0.5, 0.8), &policy());
        assert_eq!(d, ScaleDecision::ScaleUp { to: 4 });
    }

    #[test]
    fn saturated_but_out_of_budget_recommends_hardware() {
        // High load, budget headroom below the floor → cannot afford a replica.
        let d = autoscale(3, &signals(0.95, 0.05, 0.8), &policy());
        assert_eq!(d, ScaleDecision::RecommendHardware);
    }

    #[test]
    fn saturated_at_the_ceiling_recommends_hardware() {
        let p = policy();
        let d = autoscale(p.max_replicas, &signals(0.99, 0.9, 0.9), &p);
        assert_eq!(d, ScaleDecision::RecommendHardware);
    }

    #[test]
    fn idle_consolidates_down() {
        let d = autoscale(4, &signals(0.05, 0.9, 0.9), &policy());
        assert_eq!(
            d,
            ScaleDecision::ScaleDown {
                to: 3,
                reason: ScaleReason::Idle
            }
        );
    }

    #[test]
    fn budget_inefficient_scales_down_even_when_not_idle() {
        // Middling load but ROI under the floor → stop spending on a replica.
        let d = autoscale(4, &signals(0.5, 0.5, 0.1), &policy());
        assert_eq!(
            d,
            ScaleDecision::ScaleDown {
                to: 3,
                reason: ScaleReason::BudgetInefficient
            }
        );
    }

    #[test]
    fn steady_load_holds() {
        let d = autoscale(3, &signals(0.5, 0.5, 0.8), &policy());
        assert_eq!(d, ScaleDecision::Hold);
    }

    #[test]
    fn never_scales_below_min() {
        let p = policy(); // min 1
        assert_eq!(
            autoscale(1, &signals(0.0, 0.9, 0.9), &p),
            ScaleDecision::Hold
        );
        assert_eq!(
            autoscale(1, &signals(0.5, 0.9, 0.0), &p),
            ScaleDecision::Hold
        );
    }

    #[test]
    fn saturation_outranks_low_roi() {
        // Even with poor ROI, a saturated workload's immediate need is capacity;
        // scale up when affordable rather than shrink into the overload.
        let d = autoscale(3, &signals(0.9, 0.9, 0.1), &policy());
        assert_eq!(d, ScaleDecision::ScaleUp { to: 4 });
    }

    #[test]
    fn is_deterministic() {
        let s = signals(0.9, 0.5, 0.8);
        assert_eq!(autoscale(3, &s, &policy()), autoscale(3, &s, &policy()));
    }
}
