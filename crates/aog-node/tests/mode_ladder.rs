//! Cutover-ladder gate — "an identical estate differs across modes; shadow
//! never disrupts."
//!
//! A replica runs as a **real process** under the hand-managed path. The same
//! desired actions (stop the legacy replica, start its replacement) are then
//! replayed through the actuation seam on each rung of the ladder:
//!
//! * **Shadow** — the seam journals the actions and keeps coherent
//!   bookkeeping, but the real process is untouched: shadow never disrupts.
//! * **Report-only** — still untouched, and the journal surfaces as the
//!   operator's divergence report: what Loom would have done.
//! * **Enforce** — the real process is actually stopped and the replacement
//!   actually started.
//!
//! Same declared estate, three rungs, three different observable outcomes —
//! and the recorded intent is identical on every rung.

use std::sync::Arc;

use aog_estate::PolicyMode;
use aog_node::driver::{
    DriverAction, ModeGatedDriver, ProcessDriver, WorkloadDriver, WorkloadRun, WorkloadState,
};
use aog_node::probes;

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

/// The estate's desired actions, replayed identically on every rung: retire
/// the legacy replica, run its replacement.
fn replay(
    rung: &ModeGatedDriver<ProcessDriver>,
    legacy: &aog_node::driver::WorkloadHandle,
) -> aog_node::driver::WorkloadHandle {
    rung.stop(legacy).expect("stop through the seam");
    rung.start(&sleeper("gw-next"))
        .expect("start through the seam")
}

#[test]
fn an_identical_estate_differs_across_modes_and_shadow_never_disrupts() {
    let real = Arc::new(ProcessDriver::default());

    // ── The hand-managed estate: a real replica, running as a real process.
    let legacy = real.start(&sleeper("gw-legacy")).expect("legacy replica");
    assert_eq!(
        real.inspect(&legacy).expect("inspect legacy"),
        WorkloadState::Running,
        "the hand-managed replica is live before the ladder"
    );

    // ── Rung 1: Shadow — observe/reconcile, never act.
    let shadow = ModeGatedDriver::new(Arc::clone(&real), PolicyMode::Shadow);
    let shadow_next = replay(&shadow, &legacy);
    assert_eq!(
        real.inspect(&legacy).expect("inspect legacy"),
        WorkloadState::Running,
        "shadow never disrupts: the real replica the estate would retire is untouched"
    );
    assert_eq!(
        shadow.inspect(&shadow_next).expect("inspect shadow next"),
        WorkloadState::Running,
        "shadow bookkeeping is coherent: the would-be replacement reads Running"
    );
    // A real reconcile consumer (the liveness probe) composes with the rung:
    // its restart of a not-running instance reaches the journal, not the host.
    let stopped = shadow.start(&sleeper("gw-probe")).expect("bookkept start");
    shadow.stop(&stopped).expect("bookkept stop");
    let revived = probes::keep_live(&shadow, &sleeper("gw-probe"), &stopped).expect("keep_live");
    assert_eq!(
        shadow.inspect(&revived).expect("inspect revived"),
        WorkloadState::Running,
        "the probe's restart is bookkept on the shadow rung"
    );
    assert!(
        shadow.report().is_empty(),
        "shadow observes silently — the divergence report belongs to report-only"
    );
    let shadow_intent: Vec<DriverAction> = shadow.journal().into_iter().take(2).collect();

    // ── Rung 2: Report-only — still never acts, and surfaces the divergence.
    let report_only = ModeGatedDriver::new(Arc::clone(&real), PolicyMode::ReportOnly);
    let _ = replay(&report_only, &legacy);
    assert_eq!(
        real.inspect(&legacy).expect("inspect legacy"),
        WorkloadState::Running,
        "report-only does not act either"
    );
    assert_eq!(
        report_only.report(),
        vec![
            DriverAction::Stop("gw-legacy".to_owned()),
            DriverAction::Start("gw-next".to_owned()),
        ],
        "report-only surfaces exactly what Loom would have done"
    );

    // ── Rung 3: Enforce — the same estate now actually converges.
    let enforce = ModeGatedDriver::new(Arc::clone(&real), PolicyMode::Enforce);
    let next = replay(&enforce, &legacy);
    assert_ne!(
        real.inspect(&legacy).expect("inspect legacy"),
        WorkloadState::Running,
        "enforce retired the legacy replica for real"
    );
    assert_eq!(
        real.inspect(&next).expect("inspect next"),
        WorkloadState::Running,
        "enforce started the replacement for real"
    );
    assert!(
        enforce.report().is_empty(),
        "enforce acts — its record is the receipts, not a would-have report"
    );

    // ── Identical estate across the rungs: the recorded intent is the same;
    // only the mode decided whether the world changed.
    let enforce_intent: Vec<DriverAction> = enforce.journal();
    assert_eq!(shadow_intent, enforce_intent, "same intent on every rung");
    assert_eq!(
        report_only.journal(),
        enforce_intent,
        "same intent on every rung"
    );

    // Cleanup: stop the real replacement.
    enforce.stop(&next).expect("cleanup");
}
