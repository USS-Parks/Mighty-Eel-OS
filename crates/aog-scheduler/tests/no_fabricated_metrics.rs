//! The S1 gate — the defect purge, proven.
//!
//! Two obligations from the roster: **no fabricated metric in any code path
//! (audit + test)**, and **scheduler decisions trace to real inputs**. The
//! source audit walks this crate's own `src/` and asserts no fabrication API
//! (RNG, synthetic generators) is referenced anywhere. The behavioural tests
//! prove a node with no real signal is fail-closed, and that a real decision's
//! provenance mirrors the exact estate signals it read.

use std::fs;
use std::path::Path;

use aog_estate::{
    AttestationProfile, Capacity, Node, NodeSpec, NodeStatus, Workload, WorkloadKind, WorkloadSpec,
};
use aog_scheduler::{NodeSnapshot, ScheduleOutcome, ScheduleRequest, baseline_scheduler};
use fabric_contracts::Classification;

fn workload(ring: u8, ceiling: Classification) -> Workload {
    Workload::new(
        "gw",
        WorkloadSpec {
            workload_kind: WorkloadKind::Gateway,
            replicas: 1,
            ring,
            classification_ceiling: ceiling,
            image: None,
            command: Vec::new(),
            capability: None,
        },
    )
}

fn ready_node(name: &str, resource_version: u64) -> Node {
    let mut node = Node::new(
        name,
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: Capacity::default(),
        },
    );
    node.metadata.resource_version = resource_version;
    node.status = Some(NodeStatus {
        ready: true,
        last_heartbeat: Some("2026-07-04T00:00:00Z".to_owned()),
        allocatable: Capacity {
            cpu_millis: 8000,
            memory_mb: 16384,
            gpu: 2,
            max_workloads: 8,
        },
        ..NodeStatus::default()
    });
    node
}

fn visit(dir: &Path, found: &mut Vec<String>, forbidden: &[&str]) {
    for entry in fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            visit(&path, found, forbidden);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let body = fs::read_to_string(&path).expect("read rs file");
            for pat in forbidden {
                if body.contains(pat) {
                    found.push(format!("{}: {pat}", path.display()));
                }
            }
        }
    }
}

/// Audit half of the gate: no fabrication API may appear anywhere in `src/`.
/// Any of these would mean a metric could be invented rather than read from the
/// estate. The scheduler is deterministic and estate-driven; none may appear.
#[test]
fn source_has_no_fabrication_apis() {
    const FORBIDDEN: &[&str] = &[
        "thread_rng",
        "gen_range",
        "rand::",
        "fastrand",
        "SmallRng",
        "StdRng",
        "getrandom",
    ];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    visit(&src, &mut found, FORBIDDEN);
    assert!(
        found.is_empty(),
        "fabrication API referenced in scheduler src: {found:?}"
    );
}

/// A node that has never reported (no `status` at all) must be fail-closed —
/// even when its *spec* capacity is generous, its reported *allocatable* is
/// zero and it is not ready, so it is never placed.
#[test]
fn node_without_status_is_never_placed() {
    let mut silent = Node::new(
        "silent",
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: Capacity {
                cpu_millis: 99_999,
                memory_mb: 99_999,
                gpu: 8,
                max_workloads: 99,
            },
        },
    );
    silent.metadata.resource_version = 3;

    let snap = NodeSnapshot::from_node(&silent);
    assert!(!snap.ready);
    assert_eq!(
        snap.allocatable,
        Capacity::default(),
        "spec capacity must not leak in as reported allocatable"
    );

    let request = ScheduleRequest::from_workload(&workload(1, Classification::Public));
    let decision = baseline_scheduler().schedule(&request, &[snap]);
    assert!(matches!(decision.outcome, ScheduleOutcome::Pending { .. }));
}

/// The traceability half: a real decision's provenance mirrors the exact estate
/// signals — resource version, reconciled readiness, reported allocatable —
/// with nothing invented.
#[test]
fn decision_traces_to_real_signals() {
    let snap = NodeSnapshot::from_node(&ready_node("real", 11));
    let request = ScheduleRequest::from_workload(&workload(1, Classification::Public));
    let decision = baseline_scheduler().schedule(&request, std::slice::from_ref(&snap));

    assert_eq!(decision.scheduled_node(), Some("real"));
    let eval = decision
        .evaluated
        .iter()
        .find(|e| e.signals.node == "real")
        .expect("evaluation present");
    assert_eq!(eval.signals.resource_version, 11);
    assert!(eval.signals.ready);
    assert!(eval.signals.heartbeat_present);
    assert_eq!(eval.signals.allocatable.gpu, 2);
    assert!(eval.score.is_some());
}

/// A reporting node beats a silent one, and the winning binding renders to a
/// `PlacementSpec` with an as-yet-unminted token (minted at S7).
#[test]
fn only_reporting_nodes_win_and_bind() {
    let up = NodeSnapshot::from_node(&ready_node("up", 5));
    let down = NodeSnapshot::from_node(&Node::new(
        "down",
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: Capacity::default(),
        },
    ));

    let request = ScheduleRequest::from_workload(&workload(1, Classification::Public));
    let decision = baseline_scheduler().schedule(&request, &[down, up]);

    assert_eq!(decision.scheduled_node(), Some("up"));
    let placement = decision.to_placement_spec().expect("bound to a placement");
    assert_eq!(placement.node, "up");
    assert_eq!(placement.workload, "gw");
    assert!(
        placement.token_id.is_empty(),
        "the runtime token is minted at binding (S7), not at selection"
    );
}
