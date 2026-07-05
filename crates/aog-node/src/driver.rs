//! N3 — the workload driver trait (CRI-shaped). A node runs an assigned workload
//! replica through a pluggable [`WorkloadDriver`]: `start`, `inspect`, `stop`.
//! The process/systemd driver (N4) and the optional containerd driver (N5) are
//! impls behind this trait, so a workload's lifecycle is identical whichever
//! runs it. This module ships the trait, the shared value types, and
//! [`NoopDriver`] (a bookkeeping driver for shadow mode and tests).

use std::collections::HashMap;
use std::sync::Mutex;

/// What the node needs to run one workload replica (projected from its estate
/// `Workload` + `Placement`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadRun {
    /// A stable per-replica name (the placement name).
    pub name: String,
    /// Container image, when the driver launches one.
    pub image: Option<String>,
    /// Command + args (process / exec drivers).
    pub command: Vec<String>,
}

/// A handle to a started workload instance, returned by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadHandle {
    /// The run this handle belongs to.
    pub name: String,
    /// Driver-specific instance id (a PID, a container id, …).
    pub instance_id: String,
}

/// The lifecycle state of a managed workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadState {
    /// Started and running.
    Running,
    /// Exited with a status code.
    Exited(i32),
    /// The driver could not keep it running.
    Failed(String),
}

/// A CRI-shaped workload runtime. Object-safe, so a node holds a
/// `Box<dyn WorkloadDriver>` and swaps process / containerd / wasmtime impls
/// without the rest of the runtime changing.
pub trait WorkloadDriver: Send + Sync {
    /// Stable driver name (diagnostics + parity assertions).
    fn name(&self) -> &'static str;
    /// Start `run`, returning a handle to the instance.
    ///
    /// # Errors
    /// [`DriverError::Start`] if the instance cannot be launched.
    fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError>;
    /// The current state of the instance.
    ///
    /// # Errors
    /// [`DriverError::Inspect`] if the instance cannot be queried.
    fn inspect(&self, handle: &WorkloadHandle) -> Result<WorkloadState, DriverError>;
    /// Stop the instance. Idempotent: stopping an already-stopped instance is Ok.
    ///
    /// # Errors
    /// [`DriverError::Stop`] if the instance cannot be stopped.
    fn stop(&self, handle: &WorkloadHandle) -> Result<(), DriverError>;
}

/// A bookkeeping driver: it launches nothing real but tracks lifecycle in
/// memory, so shadow mode (X4) and tests can exercise the trait. A started
/// instance reads `Running` until it is stopped.
#[derive(Debug, Default)]
pub struct NoopDriver {
    running: Mutex<HashMap<String, bool>>,
}

impl WorkloadDriver for NoopDriver {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError> {
        self.running
            .lock()
            .expect("noop driver lock")
            .insert(run.name.clone(), true);
        Ok(WorkloadHandle {
            name: run.name.clone(),
            instance_id: format!("noop:{}", run.name),
        })
    }

    fn inspect(&self, handle: &WorkloadHandle) -> Result<WorkloadState, DriverError> {
        let running = self.running.lock().expect("noop driver lock");
        match running.get(&handle.name) {
            Some(true) => Ok(WorkloadState::Running),
            _ => Ok(WorkloadState::Exited(0)),
        }
    }

    fn stop(&self, handle: &WorkloadHandle) -> Result<(), DriverError> {
        self.running
            .lock()
            .expect("noop driver lock")
            .insert(handle.name.clone(), false);
        Ok(())
    }
}

/// A workload driver operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The instance could not be launched.
    #[error("driver failed to start {name:?}: {reason}")]
    Start {
        /// The run that failed to start.
        name: String,
        /// Why it failed.
        reason: String,
    },
    /// The instance could not be queried.
    #[error("driver failed to inspect {name:?}: {reason}")]
    Inspect {
        /// The instance being inspected.
        name: String,
        /// Why it failed.
        reason: String,
    },
    /// The instance could not be stopped.
    #[error("driver failed to stop {name:?}: {reason}")]
    Stop {
        /// The instance being stopped.
        name: String,
        /// Why it failed.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second, distinct driver impl — stateless, always-running — to prove the
    /// trait abstracts more than one runtime.
    struct EchoDriver;
    impl WorkloadDriver for EchoDriver {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError> {
            Ok(WorkloadHandle {
                name: run.name.clone(),
                instance_id: format!("echo:{}", run.name),
            })
        }
        fn inspect(&self, _handle: &WorkloadHandle) -> Result<WorkloadState, DriverError> {
            Ok(WorkloadState::Running)
        }
        fn stop(&self, _handle: &WorkloadHandle) -> Result<(), DriverError> {
            Ok(())
        }
    }

    fn run() -> WorkloadRun {
        WorkloadRun {
            name: "gw-node-a".to_owned(),
            image: None,
            command: vec!["gateway".to_owned()],
        }
    }

    fn starts_and_runs(driver: &dyn WorkloadDriver) {
        let handle = driver.start(&run()).expect("start");
        assert_eq!(handle.name, "gw-node-a");
        assert_eq!(
            driver.inspect(&handle).expect("inspect"),
            WorkloadState::Running
        );
    }

    #[test]
    fn the_same_workload_runs_via_two_drivers() {
        starts_and_runs(&NoopDriver::default());
        starts_and_runs(&EchoDriver);
    }

    #[test]
    fn noop_driver_reflects_stop() {
        let driver = NoopDriver::default();
        let handle = driver.start(&run()).expect("start");
        driver.stop(&handle).expect("stop");
        assert_ne!(
            driver.inspect(&handle).expect("inspect"),
            WorkloadState::Running
        );
    }
}
