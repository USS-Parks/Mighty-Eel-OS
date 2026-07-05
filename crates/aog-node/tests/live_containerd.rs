//! N5 gate — a containerized workload lifecycle via the containerd driver, with
//! parity to the process driver (N4). Env-gated on `LOOM_CONTAINER_CLI` (e.g.
//! `docker` or `nerdctl`); skips when no container CLI is configured, so it is
//! inert on the air-gap appliance path where the process driver is the default.
#![allow(clippy::print_stderr)]

use aog_node::containerd::ContainerdDriver;
use aog_node::driver::{WorkloadDriver, WorkloadHandle, WorkloadRun, WorkloadState};

#[test]
fn a_containerized_workload_has_a_lifecycle() {
    let Ok(cli) = std::env::var("LOOM_CONTAINER_CLI") else {
        eprintln!(
            "SKIP a_containerized_workload_has_a_lifecycle: LOOM_CONTAINER_CLI unset (N5 gate)"
        );
        return;
    };
    let image = std::env::var("LOOM_CONTAINER_IMAGE").unwrap_or_else(|_| "alpine".to_owned());
    let driver = ContainerdDriver::new(cli);
    let run = WorkloadRun {
        name: "loom-n5-gw".to_owned(),
        image: Some(image),
        command: vec!["sleep".to_owned(), "60".to_owned()],
    };

    // Clear any container left by a prior run (rm -f is idempotent).
    let leftover = WorkloadHandle {
        name: run.name.clone(),
        instance_id: String::new(),
    };
    driver.stop(&leftover).expect("cleanup");

    // Start → running.
    let handle = driver.start(&run).expect("start container");
    assert_eq!(driver.inspect(&handle).unwrap(), WorkloadState::Running);

    // Stop → not running.
    driver.stop(&handle).expect("stop");
    assert_ne!(driver.inspect(&handle).unwrap(), WorkloadState::Running);

    // Clean restart under the same name (parity with the process driver, N4).
    let restarted = driver.start(&run).expect("restart container");
    assert_eq!(driver.inspect(&restarted).unwrap(), WorkloadState::Running);
    driver.stop(&restarted).expect("final stop");
}
