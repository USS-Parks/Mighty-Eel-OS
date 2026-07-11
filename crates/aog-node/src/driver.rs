//! The workload driver trait (CRI-shaped). A node runs an assigned workload
//! replica through a pluggable [`WorkloadDriver`]: `start`, `inspect`, `stop`.
//! The process/systemd driver and the optional containerd driver are
//! impls behind this trait, so a workload's lifecycle is identical whichever
//! runs it. This module ships the trait, the shared value types,
//! [`NoopDriver`] (a bookkeeping driver for shadow mode and tests), and
//! [`ModeGatedDriver`] — the shadow → report-only → enforce cutover ladder
//! over the actuation seam.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use aog_estate::PolicyMode;

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

/// One lifecycle action that reached the actuation seam — journaled by
/// [`ModeGatedDriver`] whether or not the mode let it act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverAction {
    /// A start of the named replica was requested.
    Start(String),
    /// A stop of the named replica was requested.
    Stop(String),
}

/// The shadow-then-cutover ladder over the actuation seam, mirroring the
/// gateway's policy-mode ladder ([`PolicyMode`]): every consumer of the driver
/// trait (probes, drain, attestation-eviction, the node loop) routes lifecycle
/// actions through `start`/`stop`, so gating this one seam gates the whole
/// runtime.
///
/// * [`PolicyMode::Shadow`] — observe/reconcile, **never act**: actions are
///   journaled and tracked against an internal bookkeeping driver so the
///   reconcile logic sees coherent lifecycle state, but the real runtime is
///   never touched. A shadow rung cannot disrupt a hand-managed estate.
/// * [`PolicyMode::ReportOnly`] — as Shadow, and the journal is the operator's
///   divergence report ([`Self::report`]): what Loom *would have done*.
/// * [`PolicyMode::Enforce`] — actions reach the real driver.
///
/// Unlike the data-path ladder (where the non-blocking modes are
/// development-only, because not blocking a classified egress is unsafe), the
/// orchestration ladder is a **production migration procedure**: shadow is safe
/// by construction — it never disrupts — so a live estate steps
/// shadow → report-only → enforce during cutover. Stepping a rung is
/// constructing the next [`ModeGatedDriver`] over the **same** real driver.
pub struct ModeGatedDriver<D: WorkloadDriver> {
    real: Arc<D>,
    mode: PolicyMode,
    /// Coherent lifecycle bookkeeping for the non-acting rungs, so reconcile
    /// logic driving this seam observes the states it expects.
    shadow: NoopDriver,
    journal: Mutex<Vec<DriverAction>>,
}

impl<D: WorkloadDriver> ModeGatedDriver<D> {
    /// Gate `real` behind `mode`. The same `real` instance threads through
    /// successive rungs of the ladder.
    #[must_use]
    pub fn new(real: Arc<D>, mode: PolicyMode) -> Self {
        Self {
            real,
            mode,
            shadow: NoopDriver::default(),
            journal: Mutex::new(Vec::new()),
        }
    }

    /// The rung this driver is on.
    #[must_use]
    pub fn mode(&self) -> PolicyMode {
        self.mode
    }

    /// Every action that reached the seam on this rung, in order.
    #[must_use]
    pub fn journal(&self) -> Vec<DriverAction> {
        self.journal.lock().expect("journal lock").clone()
    }

    /// The operator-facing divergence report: on the report-only rung, the
    /// actions Loom would have taken. Empty on the other rungs — shadow
    /// observes silently, and enforce actually acts (its record is the
    /// receipts, not a would-have report).
    #[must_use]
    pub fn report(&self) -> Vec<DriverAction> {
        match self.mode {
            PolicyMode::ReportOnly => self.journal(),
            PolicyMode::Shadow | PolicyMode::Enforce => Vec::new(),
        }
    }

    fn acts(&self) -> bool {
        matches!(self.mode, PolicyMode::Enforce)
    }
}

impl<D: WorkloadDriver> WorkloadDriver for ModeGatedDriver<D> {
    fn name(&self) -> &'static str {
        // The ladder is transparent: parity assertions see the real runtime.
        self.real.name()
    }

    fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError> {
        self.journal
            .lock()
            .expect("journal lock")
            .push(DriverAction::Start(run.name.clone()));
        if self.acts() {
            self.real.start(run)
        } else {
            self.shadow.start(run)
        }
    }

    fn inspect(&self, handle: &WorkloadHandle) -> Result<WorkloadState, DriverError> {
        if self.acts() {
            self.real.inspect(handle)
        } else {
            self.shadow.inspect(handle)
        }
    }

    fn stop(&self, handle: &WorkloadHandle) -> Result<(), DriverError> {
        self.journal
            .lock()
            .expect("journal lock")
            .push(DriverAction::Stop(handle.name.clone()));
        if self.acts() {
            self.real.stop(handle)
        } else {
            self.shadow.stop(handle)
        }
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
