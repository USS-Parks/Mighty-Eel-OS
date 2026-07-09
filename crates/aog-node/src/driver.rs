//! The workload driver trait (CRI-shaped). A node runs an assigned workload
//! replica through a pluggable [`WorkloadDriver`]: `start`, `inspect`, `stop`.
//! The process/systemd driver and the optional containerd driver are
//! impls behind this trait, so a workload's lifecycle is identical whichever
//! runs it. This module ships the trait, the shared value types, and
//! [`NoopDriver`] (a bookkeeping driver for shadow mode and tests).

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
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

/// The process driver: it runs a workload replica as a real child process.
/// This is the **air-gap appliance default** — no container runtime required. On
/// Linux, production wraps it in a systemd unit for boot-time supervision and
/// restart-on-failure; the lifecycle this driver provides (start / inspect /
/// stop / clean restart) is the same regardless of the service manager on top.
#[derive(Debug, Default)]
pub struct ProcessDriver {
    instances: Mutex<HashMap<String, Child>>,
}

impl WorkloadDriver for ProcessDriver {
    fn name(&self) -> &'static str {
        "process"
    }

    fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError> {
        let Some((program, args)) = run.command.split_first() else {
            return Err(DriverError::Start {
                name: run.name.clone(),
                reason: "empty command".to_owned(),
            });
        };
        let mut instances = self.instances.lock().expect("process driver lock");
        // Reap any prior instance under this name so a restart never leaks a PID.
        if let Some(mut old) = instances.remove(&run.name) {
            let _ = old.kill();
            let _ = old.wait();
        }
        let child = Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DriverError::Start {
                name: run.name.clone(),
                reason: e.to_string(),
            })?;
        let instance_id = child.id().to_string();
        instances.insert(run.name.clone(), child);
        Ok(WorkloadHandle {
            name: run.name.clone(),
            instance_id,
        })
    }

    fn inspect(&self, handle: &WorkloadHandle) -> Result<WorkloadState, DriverError> {
        let mut instances = self.instances.lock().expect("process driver lock");
        let Some(child) = instances.get_mut(&handle.name) else {
            return Ok(WorkloadState::Exited(0)); // not tracked — already stopped
        };
        match child.try_wait() {
            Ok(Some(status)) => Ok(WorkloadState::Exited(status.code().unwrap_or(-1))),
            Ok(None) => Ok(WorkloadState::Running),
            Err(e) => Err(DriverError::Inspect {
                name: handle.name.clone(),
                reason: e.to_string(),
            }),
        }
    }

    fn stop(&self, handle: &WorkloadHandle) -> Result<(), DriverError> {
        let mut instances = self.instances.lock().expect("process driver lock");
        if let Some(mut child) = instances.remove(&handle.name) {
            let _ = child.kill(); // ignore "already exited"
            let _ = child.wait(); // reap the zombie
        }
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

    /// A long-running child, portable across the test host's OS. Killed by the
    /// test, so it never runs to completion.
    fn sleeper(name: &str) -> WorkloadRun {
        let command = if cfg!(windows) {
            vec![
                "ping".to_owned(),
                "-n".to_owned(),
                "30".to_owned(),
                "127.0.0.1".to_owned(),
            ]
        } else {
            vec!["sleep".to_owned(), "30".to_owned()]
        };
        WorkloadRun {
            name: name.to_owned(),
            image: None,
            command,
        }
    }

    #[test]
    fn a_gateway_replica_has_a_process_lifecycle() {
        let driver = ProcessDriver::default();
        let run = sleeper("gw-node-a");

        let handle = driver.start(&run).expect("start");
        assert_eq!(
            driver.inspect(&handle).expect("inspect"),
            WorkloadState::Running
        );

        driver.stop(&handle).expect("stop");
        assert_ne!(
            driver.inspect(&handle).expect("inspect"),
            WorkloadState::Running
        );

        // Clean restart under the same name.
        let restarted = driver.start(&run).expect("restart");
        assert_eq!(
            driver.inspect(&restarted).expect("inspect"),
            WorkloadState::Running
        );
        driver.stop(&restarted).expect("stop");
    }

    #[test]
    fn an_empty_command_fails_to_start() {
        let driver = ProcessDriver::default();
        let run = WorkloadRun {
            name: "x".to_owned(),
            image: None,
            command: vec![],
        };
        assert!(matches!(driver.start(&run), Err(DriverError::Start { .. })));
    }
}
