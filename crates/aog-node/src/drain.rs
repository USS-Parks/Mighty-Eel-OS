//! N9 — eviction + drain. A planned eviction (maintenance, reschedule) drains
//! **gracefully**: it gates new traffic (readiness → false, N7), lets in-flight
//! authorized calls finish, and honours the workload's disruption budget so no
//! more replicas are unavailable at once than allowed. A **Tier-0 revocation**
//! drains **immediately** — a compromised or killed workload halts now, in-flight
//! or not (I-9): the safety of stopping outweighs a dropped call.

use crate::driver::{DriverError, WorkloadDriver, WorkloadHandle};

/// Why a workload is being drained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainReason {
    /// Planned — drain gracefully within the disruption budget.
    Planned,
    /// Tier-0 revocation (kill switch / compromise) — drain immediately.
    Revocation,
}

/// A PodDisruptionBudget-analog: how many replicas of a workload may be
/// unavailable at once, and how many already are.
#[derive(Debug, Clone, Copy)]
pub struct DisruptionBudget {
    /// The most replicas that may be unavailable simultaneously.
    pub max_unavailable: u32,
    /// How many are unavailable right now.
    pub currently_unavailable: u32,
}

impl DisruptionBudget {
    /// Whether one more replica may be taken down without breaching the budget.
    #[must_use]
    pub fn has_room(&self) -> bool {
        self.currently_unavailable < self.max_unavailable
    }
}

/// What the node should do to drain one instance now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainAction {
    /// Stop immediately — a revocation drain drops nothing to policy (I-9).
    ForceStopNow,
    /// No in-flight work: gate traffic and stop.
    StopNow,
    /// In-flight authorized calls remain: keep the instance up (traffic gated)
    /// until they finish, then stop — they are not dropped.
    WaitForInflight {
        /// How many authorized calls are still in flight.
        remaining: u64,
    },
    /// The disruption budget is exhausted: defer this graceful drain.
    Defer,
}

/// Decide how to drain an instance given the `reason`, its in-flight authorized
/// call count, and the workload's disruption `budget`.
///
/// A revocation is unconditional and immediate — it ignores both the in-flight
/// count and the budget. A planned drain defers when the budget has no room,
/// waits for in-flight calls to finish otherwise, and stops when there are none.
#[must_use]
pub fn plan_drain(reason: DrainReason, inflight: u64, budget: DisruptionBudget) -> DrainAction {
    match reason {
        DrainReason::Revocation => DrainAction::ForceStopNow,
        DrainReason::Planned => {
            if !budget.has_room() {
                DrainAction::Defer
            } else if inflight > 0 {
                DrainAction::WaitForInflight {
                    remaining: inflight,
                }
            } else {
                DrainAction::StopNow
            }
        }
    }
}

/// Execute a drain `action` against the driver: stop the instance for a
/// `StopNow` or `ForceStopNow`; leave it running (traffic gated by the caller,
/// N7) for a `WaitForInflight` or `Defer`. Returns whether the instance was
/// stopped.
///
/// # Errors
/// Propagates a [`DriverError`] from the stop.
pub fn execute_drain(
    driver: &dyn WorkloadDriver,
    handle: &WorkloadHandle,
    action: &DrainAction,
) -> Result<bool, DriverError> {
    match action {
        DrainAction::ForceStopNow | DrainAction::StopNow => {
            driver.stop(handle)?;
            Ok(true)
        }
        DrainAction::WaitForInflight { .. } | DrainAction::Defer => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{NoopDriver, WorkloadDriver, WorkloadRun, WorkloadState};

    fn budget(max: u32, current: u32) -> DisruptionBudget {
        DisruptionBudget {
            max_unavailable: max,
            currently_unavailable: current,
        }
    }

    fn started(driver: &NoopDriver, name: &str) -> WorkloadHandle {
        driver
            .start(&WorkloadRun {
                name: name.to_owned(),
                image: None,
                command: vec!["gateway".to_owned()],
            })
            .unwrap()
    }

    #[test]
    fn a_graceful_drain_waits_for_in_flight_calls() {
        assert_eq!(
            plan_drain(DrainReason::Planned, 3, budget(1, 0)),
            DrainAction::WaitForInflight { remaining: 3 }
        );
    }

    #[test]
    fn a_graceful_drain_with_no_in_flight_stops() {
        assert_eq!(
            plan_drain(DrainReason::Planned, 0, budget(1, 0)),
            DrainAction::StopNow
        );
    }

    #[test]
    fn a_graceful_drain_defers_when_the_budget_is_exhausted() {
        assert_eq!(
            plan_drain(DrainReason::Planned, 0, budget(1, 1)),
            DrainAction::Defer
        );
    }

    #[test]
    fn a_revocation_drain_is_immediate_regardless_of_in_flight_or_budget() {
        assert_eq!(
            plan_drain(DrainReason::Revocation, 10, budget(0, 5)),
            DrainAction::ForceStopNow
        );
    }

    #[test]
    fn a_revocation_drain_stops_the_instance_immediately() {
        let driver = NoopDriver::default();
        let handle = started(&driver, "gw-a");
        let action = plan_drain(DrainReason::Revocation, 7, budget(1, 0));
        assert!(execute_drain(&driver, &handle, &action).unwrap());
        assert_ne!(driver.inspect(&handle).unwrap(), WorkloadState::Running);
    }

    #[test]
    fn a_graceful_drain_with_in_flight_does_not_stop() {
        let driver = NoopDriver::default();
        let handle = started(&driver, "gw-a");
        let action = plan_drain(DrainReason::Planned, 2, budget(1, 0));
        assert!(
            !execute_drain(&driver, &handle, &action).unwrap(),
            "in-flight authorized calls are not dropped"
        );
        assert_eq!(driver.inspect(&handle).unwrap(), WorkloadState::Running);
    }
}
