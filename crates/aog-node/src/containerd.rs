//! The containerd driver (optional). Runs a workload replica as a container
//! through a containerd-compatible CLI (`nerdctl` / `ctr`; also `docker`, which
//! is containerd-backed). It satisfies the same [`WorkloadDriver`] trait as the
//! process driver — a workload's lifecycle is identical whichever runs it,
//! the parity the trait guarantees. On the air-gap appliance the process
//! driver is the default; this driver is for hosts already running containerd.

use std::process::Command;

use crate::driver::{DriverError, WorkloadDriver, WorkloadHandle, WorkloadRun, WorkloadState};

/// Runs workloads as containers via a containerd-compatible CLI.
#[derive(Debug, Clone)]
pub struct ContainerdDriver {
    cli: String,
}

impl ContainerdDriver {
    /// Build a driver that shells out to `cli` (`nerdctl`, `ctr`, or `docker`).
    #[must_use]
    pub fn new(cli: impl Into<String>) -> Self {
        Self { cli: cli.into() }
    }

    /// The default appliance-adjacent containerd CLI, `nerdctl`.
    #[must_use]
    pub fn nerdctl() -> Self {
        Self::new("nerdctl")
    }
}

/// The CLI args to start `run` as a detached, named container.
fn run_args(run: &WorkloadRun) -> Result<Vec<String>, DriverError> {
    let image = run.image.as_deref().ok_or_else(|| DriverError::Start {
        name: run.name.clone(),
        reason: "containerd driver requires an image".to_owned(),
    })?;
    let mut args = vec![
        "run".to_owned(),
        "-d".to_owned(),
        "--name".to_owned(),
        run.name.clone(),
        image.to_owned(),
    ];
    args.extend(run.command.iter().cloned());
    Ok(args)
}

/// The CLI args to read whether the named container is running.
fn inspect_args(name: &str) -> Vec<String> {
    vec![
        "inspect".to_owned(),
        "-f".to_owned(),
        "{{.State.Running}}".to_owned(),
        name.to_owned(),
    ]
}

/// The CLI args to force-remove the named container (idempotent).
fn stop_args(name: &str) -> Vec<String> {
    vec!["rm".to_owned(), "-f".to_owned(), name.to_owned()]
}

impl WorkloadDriver for ContainerdDriver {
    fn name(&self) -> &'static str {
        "containerd"
    }

    fn start(&self, run: &WorkloadRun) -> Result<WorkloadHandle, DriverError> {
        let args = run_args(run)?;
        let output =
            Command::new(&self.cli)
                .args(&args)
                .output()
                .map_err(|e| DriverError::Start {
                    name: run.name.clone(),
                    reason: e.to_string(),
                })?;
        if !output.status.success() {
            return Err(DriverError::Start {
                name: run.name.clone(),
                reason: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(WorkloadHandle {
            name: run.name.clone(),
            instance_id: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        })
    }

    fn inspect(&self, handle: &WorkloadHandle) -> Result<WorkloadState, DriverError> {
        let output = Command::new(&self.cli)
            .args(inspect_args(&handle.name))
            .output()
            .map_err(|e| DriverError::Inspect {
                name: handle.name.clone(),
                reason: e.to_string(),
            })?;
        if !output.status.success() {
            return Ok(WorkloadState::Exited(0)); // gone / not found
        }
        let running = String::from_utf8_lossy(&output.stdout)
            .trim()
            .eq_ignore_ascii_case("true");
        Ok(if running {
            WorkloadState::Running
        } else {
            WorkloadState::Exited(0)
        })
    }

    fn stop(&self, handle: &WorkloadHandle) -> Result<(), DriverError> {
        // rm -f is idempotent; a missing container is not an error.
        Command::new(&self.cli)
            .args(stop_args(&handle.name))
            .output()
            .map_err(|e| DriverError::Stop {
                name: handle.name.clone(),
                reason: e.to_string(),
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> WorkloadRun {
        WorkloadRun {
            name: "gw-node-a".to_owned(),
            image: Some("alpine".to_owned()),
            command: vec!["sleep".to_owned(), "30".to_owned()],
        }
    }

    fn owned(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn run_args_are_a_detached_named_container() {
        let args = run_args(&run()).expect("image present");
        assert_eq!(
            args,
            owned(&["run", "-d", "--name", "gw-node-a", "alpine", "sleep", "30"])
        );
    }

    #[test]
    fn run_without_an_image_fails() {
        let run = WorkloadRun {
            name: "x".to_owned(),
            image: None,
            command: vec![],
        };
        assert!(matches!(run_args(&run), Err(DriverError::Start { .. })));
    }

    #[test]
    fn inspect_and_stop_args_target_the_container() {
        assert_eq!(
            inspect_args("gw-node-a"),
            owned(&["inspect", "-f", "{{.State.Running}}", "gw-node-a"])
        );
        assert_eq!(stop_args("gw-node-a"), owned(&["rm", "-f", "gw-node-a"]));
    }
}
