//! N7 — health probes. The node supervises the workloads it runs: a **liveness**
//! probe restarts an instance the driver reports not-Running (an unhealthy
//! replica is replaced), and a **readiness** probe gates traffic — only ready
//! instances receive it. Both are pluggable, so an HTTP `/healthz` liveness or a
//! `/ready` readiness slots in behind the same seam.

use crate::driver::{DriverError, WorkloadDriver, WorkloadHandle, WorkloadRun, WorkloadState};

/// A readiness check for a running instance — is it ready to serve? Pluggable
/// (an HTTP `/ready`, a socket probe, a driver flag …).
pub trait ReadinessProbe: Send + Sync {
    /// Whether `handle`'s instance is ready to receive traffic.
    fn ready(&self, handle: &WorkloadHandle) -> bool;
}

/// Keep `run` live: if the driver reports its `handle` not Running, restart it
/// and return the fresh handle; otherwise return the handle unchanged. An
/// unhealthy replica is replaced.
///
/// # Errors
/// Propagates a [`DriverError`] from the inspect or restart.
pub fn keep_live(
    driver: &dyn WorkloadDriver,
    run: &WorkloadRun,
    handle: &WorkloadHandle,
) -> Result<WorkloadHandle, DriverError> {
    match driver.inspect(handle)? {
        WorkloadState::Running => Ok(handle.clone()),
        WorkloadState::Exited(_) | WorkloadState::Failed(_) => driver.start(run),
    }
}

/// The subset of `handles` a readiness probe reports ready — the only instances
/// traffic should reach. A not-ready instance is gated out.
#[must_use]
pub fn ready_targets<'a>(
    handles: &'a [WorkloadHandle],
    probe: &dyn ReadinessProbe,
) -> Vec<&'a WorkloadHandle> {
    handles
        .iter()
        .filter(|&handle| probe.ready(handle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::NoopDriver;

    fn run(name: &str) -> WorkloadRun {
        WorkloadRun {
            name: name.to_owned(),
            image: None,
            command: vec!["gateway".to_owned()],
        }
    }

    #[test]
    fn an_unhealthy_replica_is_restarted() {
        let driver = NoopDriver::default();
        let r = run("gw-a");
        let handle = driver.start(&r).unwrap();
        driver.stop(&handle).unwrap(); // it died
        assert_ne!(driver.inspect(&handle).unwrap(), WorkloadState::Running);

        let live = keep_live(&driver, &r, &handle).unwrap();
        assert_eq!(driver.inspect(&live).unwrap(), WorkloadState::Running);
    }

    #[test]
    fn a_healthy_replica_is_left_running() {
        let driver = NoopDriver::default();
        let r = run("gw-a");
        let handle = driver.start(&r).unwrap();

        let live = keep_live(&driver, &r, &handle).unwrap();
        assert_eq!(live, handle, "a running replica is not restarted");
        assert_eq!(driver.inspect(&live).unwrap(), WorkloadState::Running);
    }

    struct ReadyOnly(&'static str);
    impl ReadinessProbe for ReadyOnly {
        fn ready(&self, handle: &WorkloadHandle) -> bool {
            handle.name == self.0
        }
    }

    #[test]
    fn readiness_gates_traffic() {
        let handles = vec![
            WorkloadHandle {
                name: "gw-a".to_owned(),
                instance_id: "1".to_owned(),
            },
            WorkloadHandle {
                name: "gw-b".to_owned(),
                instance_id: "2".to_owned(),
            },
        ];
        let ready = ready_targets(&handles, &ReadyOnly("gw-b"));
        assert_eq!(ready.len(), 1, "only the ready instance takes traffic");
        assert_eq!(ready[0].name, "gw-b");
    }
}
