//! O2 — the rollout controller: it advances a `RolloutPlan` through steps that
//! respect the surge/unavailable budget, so a workload (or bundle / provider
//! config) is rolled without ever dropping below its availability floor, and
//! records each step in `status` — an admitted write, therefore a receipt.
//!
//! The stepping decision is a **pure function** of the strategy and budget
//! ([`rollout_progress`]) — no clock, no estate read — so a rollout is
//! deterministic (A1.12 bar-5) and its availability floor
//! (`available >= total - max_unavailable` at every step) is *provable*, not
//! hoped for. The controller is the thin loop that reads the target's size, asks
//! the stepper where the rollout is, and writes the next step until complete.
//!
//! Scope (M3c): the concrete target is a `Workload` (`total` = its replicas). A
//! `PolicyBundle` / provider-config rollout rides the same stepper — the
//! availability arithmetic is target-agnostic — and is wired when those targets
//! carry a rollout. The physical replacement of a running replica is O1's
//! placement plus the node's drain; O2 owns the *order and pace* and the receipt trail.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use aog_estate::{Kind, Phase, ResourceObject, RolloutPlan, RolloutPlanStatus, RolloutStrategy};

use crate::objects::{EstateClient, parse_key};
use crate::runtime::{Action, ReconcileError, Reconciler};

/// Cadence between rollout steps — brisk, since each step is a bounded, safe
/// advance; the informer also re-enqueues on each status write.
const REQUEUE: Duration = Duration::from_millis(200);

/// Where a rollout stands after `step` steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolloutProgress {
    /// Targets that have finished updating.
    pub updated: usize,
    /// Targets momentarily unavailable during this step — never `> max_unavailable`.
    pub unavailable: usize,
    /// The rollout has reached the full target set.
    pub complete: bool,
}

impl RolloutProgress {
    /// Available targets during this step: `total - unavailable`. By construction
    /// this never falls below `total - max_unavailable` (the availability floor).
    #[must_use]
    pub fn available(&self, total: usize) -> usize {
        total.saturating_sub(self.unavailable)
    }
}

/// Per-step replacement window: how many targets a progressive rollout cycles
/// per step. K8s-shaped: surge (extra new brought up) + unavailable (old taken
/// down), at least one so a rollout always progresses (the `RolloutPlan`
/// validator forbids both being zero).
fn window(max_surge: u32, max_unavailable: u32) -> usize {
    (max_surge as usize + max_unavailable as usize).max(1)
}

/// The canary cohort: the small first batch a canary rollout validates before
/// committing the rest. At least one target, at most the whole set.
fn canary(total: usize, max_surge: u32) -> usize {
    (max_surge as usize).clamp(1, total.max(1)).min(total)
}

/// How many targets have finished updating after `step` steps of `strategy`.
/// Monotonic non-decreasing in `step`.
fn updated_after(
    strategy: RolloutStrategy,
    total: usize,
    max_surge: u32,
    max_unavailable: u32,
    step: usize,
) -> usize {
    match strategy {
        RolloutStrategy::Progressive => (step * window(max_surge, max_unavailable)).min(total),
        RolloutStrategy::Canary => match step {
            0 => 0,
            1 => canary(total, max_surge),
            _ => total,
        },
        // Blue/green stands the whole new set up beside the old (steps 0..1) and
        // switches atomically at step 2 — nothing counts as updated until the cut.
        RolloutStrategy::BlueGreen => usize::from(step >= 2) * total,
    }
}

/// The pure stepping decision (A1.12 bar-5 determinism): given the strategy, the
/// target size `total`, the surge/unavailable budget, and the current `step`,
/// where the rollout is and how many targets are momentarily down. The
/// availability floor `total - max_unavailable` holds at every step because
/// `unavailable <= max_unavailable` by construction.
#[must_use]
pub fn rollout_progress(
    strategy: RolloutStrategy,
    total: usize,
    max_surge: u32,
    max_unavailable: u32,
    step: usize,
) -> RolloutProgress {
    if total == 0 {
        return RolloutProgress {
            updated: 0,
            unavailable: 0,
            complete: true,
        };
    }
    let updated = updated_after(strategy, total, max_surge, max_unavailable, step);
    let prev = updated_after(
        strategy,
        total,
        max_surge,
        max_unavailable,
        step.saturating_sub(1),
    );
    let batch = updated.saturating_sub(prev);
    // Blue/green never takes a target down (the old set serves until the switch);
    // progressive/canary take up to max_unavailable of the in-flight batch down.
    let unavailable = match strategy {
        RolloutStrategy::BlueGreen => 0,
        _ => (max_unavailable as usize).min(batch),
    };
    let complete = match strategy {
        RolloutStrategy::BlueGreen => step >= 2,
        _ => updated >= total,
    };
    RolloutProgress {
        updated,
        unavailable,
        complete,
    }
}

/// The terminal `status.step` a rollout reaches when complete — the number of
/// steps it takes. Used by tests and operators; the controller itself just
/// advances until [`rollout_progress`] reports `complete`.
#[must_use]
pub fn total_steps(
    strategy: RolloutStrategy,
    total: usize,
    max_surge: u32,
    max_unavailable: u32,
) -> usize {
    if total == 0 {
        return 0;
    }
    match strategy {
        RolloutStrategy::Progressive => total.div_ceil(window(max_surge, max_unavailable)),
        RolloutStrategy::Canary => usize::from(canary(total, max_surge) < total) + 1,
        RolloutStrategy::BlueGreen => 2,
    }
}

/// Observed error/failure count for a rollout's target — read from the
/// tamper-evident receipt ledger or the meter (O3). Synchronous: it reflects a
/// telemetry snapshot the caller already holds, keeping the controller's estate
/// access on the async side. A fail-closed source reports conservatively (an
/// unreadable signal is never "zero errors").
pub trait ErrorBudgetProbe: Send + Sync {
    /// Errors observed for `target` in the current rollout window.
    fn errors(&self, target: &str) -> u32;
}

/// Advances `RolloutPlan`s. Run it on a `"RolloutPlan/"` informer.
#[derive(Clone)]
pub struct RolloutController {
    client: EstateClient,
    error_probe: Option<Arc<dyn ErrorBudgetProbe>>,
}

impl RolloutController {
    #[must_use]
    pub fn new(client: EstateClient) -> Self {
        Self {
            client,
            error_probe: None,
        }
    }

    /// Enable O3 auto-rollback: when `probe` reports more errors for a rollout's
    /// target than its `error_budget`, the rollout reverses to its prior state
    /// (ending `Failed`), each reverse step receipted. Without a probe the
    /// controller is forward-only (O2).
    #[must_use]
    pub fn with_error_budget(mut self, probe: Arc<dyn ErrorBudgetProbe>) -> Self {
        self.error_probe = Some(probe);
        self
    }

    /// Whether the rollout's error budget is set and exceeded by observed errors.
    fn budget_breached(&self, plan: &RolloutPlan) -> bool {
        let Some(probe) = &self.error_probe else {
            return false;
        };
        plan.spec.error_budget > 0 && probe.errors(&plan.spec.target) > plan.spec.error_budget
    }

    /// The rollout target's size. M3c resolves a `Workload` target to its replica
    /// count; a target that is not a live workload yields `None` (the rollout is
    /// `Degraded` until its target exists — fail-closed, never a phantom rollout).
    async fn target_total(&self, plan: &RolloutPlan) -> Result<Option<usize>, ReconcileError> {
        match self.client.get(Kind::Workload, &plan.spec.target).await? {
            Some(ResourceObject::Workload(wl)) if wl.metadata.deletion_timestamp.is_none() => {
                Ok(Some(wl.spec.replicas as usize))
            }
            _ => Ok(None),
        }
    }

    /// Write `status` when it changed (level-triggered; an admitted update is a
    /// receipt).
    async fn set_status(
        &self,
        plan: RolloutPlan,
        phase: Phase,
        step: u32,
        rolled_back: bool,
    ) -> Result<(), ReconcileError> {
        let desired = RolloutPlanStatus {
            phase,
            step,
            rolled_back,
        };
        if plan.status.as_ref() != Some(&desired) {
            let mut converged = plan;
            converged.status = Some(desired);
            self.client
                .update(ResourceObject::RolloutPlan(converged))
                .await?;
        }
        Ok(())
    }

    async fn reconcile_rollout(&self, name: &str) -> Result<Action, ReconcileError> {
        let Some(ResourceObject::RolloutPlan(plan)) =
            self.client.get(Kind::RolloutPlan, name).await?
        else {
            return Ok(Action::Done);
        };
        if plan.metadata.deletion_timestamp.is_some() {
            return Ok(Action::Done);
        }

        let step = plan.status.as_ref().map_or(0, |s| s.step);
        let rolling_back = plan.status.as_ref().is_some_and(|s| s.rolled_back);
        let Some(total) = self.target_total(&plan).await? else {
            // Target absent: hold Degraded (fail-closed). Its creation won't wake
            // this informer, so a resync heartbeat re-checks — not a hot requeue
            // spin; the controller is run with `with_resync` for exactly this.
            self.set_status(plan, Phase::Degraded, step, rolling_back)
                .await?;
            return Ok(Action::Done);
        };

        // O3 — auto-rollback. Once the error budget is breached (or a rollback is
        // already under way), reverse toward step 0 — the prior state — each
        // reverse an audited receipt. A rollback in flight never un-reverses even
        // if the error signal later clears: deterministic, ledger-provable.
        if rolling_back || self.budget_breached(&plan) {
            return if step > 0 {
                self.set_status(plan, Phase::Provisioning, step - 1, true)
                    .await?;
                Ok(Action::RequeueAfter(REQUEUE))
            } else {
                // Prior state restored; the rollout has failed its budget.
                self.set_status(plan, Phase::Failed, 0, true).await?;
                Ok(Action::Done)
            };
        }

        let progress = rollout_progress(
            plan.spec.strategy,
            total,
            plan.spec.max_surge,
            plan.spec.max_unavailable,
            step as usize,
        );
        if progress.complete {
            self.set_status(plan, Phase::Ready, step, false).await?;
            return Ok(Action::Done);
        }
        // One bounded, availability-safe step forward, then come back for the next.
        self.set_status(plan, Phase::Provisioning, step + 1, false)
            .await?;
        Ok(Action::RequeueAfter(REQUEUE))
    }
}

impl Reconciler for RolloutController {
    fn reconcile(&self, key: &str) -> impl Future<Output = Result<Action, ReconcileError>> + Send {
        let controller = self.clone();
        let key = key.to_owned();
        async move {
            let Some((Kind::RolloutPlan, name)) = parse_key(&key) else {
                return Ok(Action::Done);
            };
            controller.reconcile_rollout(name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing invariant: at every step of every strategy, availability
    /// never drops below the floor `total - max_unavailable`.
    #[test]
    fn availability_floor_holds_across_all_strategies() {
        let strategies = [
            RolloutStrategy::Progressive,
            RolloutStrategy::Canary,
            RolloutStrategy::BlueGreen,
        ];
        for strategy in strategies {
            for total in 1..=12usize {
                for max_surge in 0..=4u32 {
                    for max_unavailable in 0..=4u32 {
                        if max_surge == 0 && max_unavailable == 0 {
                            continue; // the validator forbids this (would stall)
                        }
                        let floor = total.saturating_sub(max_unavailable as usize);
                        for step in 0..=total + 3 {
                            let p =
                                rollout_progress(strategy, total, max_surge, max_unavailable, step);
                            assert!(
                                p.available(total) >= floor,
                                "{strategy:?} total={total} surge={max_surge} unavail={max_unavailable} step={step}: available {} < floor {floor}",
                                p.available(total),
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn progressive_completes_in_ceil_total_over_window() {
        // total 10, window = surge2 + unavail1 = 3 → ceil(10/3) = 4 steps.
        let steps = total_steps(RolloutStrategy::Progressive, 10, 2, 1);
        assert_eq!(steps, 4);
        assert!(!rollout_progress(RolloutStrategy::Progressive, 10, 2, 1, steps - 1).complete);
        assert!(rollout_progress(RolloutStrategy::Progressive, 10, 2, 1, steps).complete);
        assert_eq!(
            rollout_progress(RolloutStrategy::Progressive, 10, 2, 1, steps).updated,
            10
        );
    }

    #[test]
    fn zero_unavailable_is_zero_downtime() {
        // maxUnavailable 0 + surge 2: progress by surge alone, nothing ever down.
        for step in 0..=4 {
            let p = rollout_progress(RolloutStrategy::Progressive, 6, 2, 0, step);
            assert_eq!(
                p.unavailable, 0,
                "surge-only rollout never takes a target down"
            );
            assert_eq!(p.available(6), 6);
        }
        assert!(rollout_progress(RolloutStrategy::Progressive, 6, 2, 0, 3).complete);
    }

    #[test]
    fn canary_validates_a_small_batch_then_the_rest() {
        // surge 1 → canary of 1, then the remaining 9 in the second step.
        assert_eq!(total_steps(RolloutStrategy::Canary, 10, 1, 1), 2);
        assert_eq!(
            rollout_progress(RolloutStrategy::Canary, 10, 1, 1, 1).updated,
            1
        );
        assert!(!rollout_progress(RolloutStrategy::Canary, 10, 1, 1, 1).complete);
        let done = rollout_progress(RolloutStrategy::Canary, 10, 1, 1, 2);
        assert_eq!(done.updated, 10);
        assert!(done.complete);
    }

    #[test]
    fn bluegreen_switches_atomically_with_no_downtime() {
        assert_eq!(total_steps(RolloutStrategy::BlueGreen, 5, 5, 0), 2);
        // Green stands up beside blue: nothing updated, nothing down, not done.
        let up = rollout_progress(RolloutStrategy::BlueGreen, 5, 5, 0, 1);
        assert_eq!(up.updated, 0);
        assert_eq!(up.unavailable, 0);
        assert!(!up.complete);
        // The switch: whole set live on green, still zero downtime.
        let switched = rollout_progress(RolloutStrategy::BlueGreen, 5, 5, 0, 2);
        assert_eq!(switched.updated, 5);
        assert_eq!(switched.unavailable, 0);
        assert!(switched.complete);
    }

    #[test]
    fn an_empty_rollout_is_immediately_complete() {
        for strategy in [
            RolloutStrategy::Progressive,
            RolloutStrategy::Canary,
            RolloutStrategy::BlueGreen,
        ] {
            assert_eq!(total_steps(strategy, 0, 1, 1), 0);
            assert!(rollout_progress(strategy, 0, 1, 1, 0).complete);
        }
    }

    #[test]
    fn progress_is_deterministic() {
        let a = rollout_progress(RolloutStrategy::Progressive, 7, 2, 1, 2);
        let b = rollout_progress(RolloutStrategy::Progressive, 7, 2, 1, 2);
        assert_eq!(a, b);
    }
}
