//! Phase-S attested-placement integration tests, exercised through
//! `attested_scheduler()` — the wiring the control plane drives. Grows across
//! S2 (capacity + utilisation), S3 (ring), S4 (attestation).

use aog_estate::{
    AttestationProfile, Capacity, Node, NodeSpec, NodeStatus, Workload, WorkloadKind, WorkloadSpec,
};
use aog_scheduler::{NodeSnapshot, ScheduleRequest, attested_scheduler};
use fabric_contracts::Classification;

fn workload() -> Workload {
    Workload::new(
        "gw",
        WorkloadSpec {
            workload_kind: WorkloadKind::Gateway,
            replicas: 1,
            ring: 1,
            classification_ceiling: Classification::Public,
            image: None,
            command: Vec::new(),
            capability: None,
        },
    )
}

/// A ready node with a declared total capacity and a reported free capacity.
fn node(name: &str, total: Capacity, free: Capacity) -> NodeSnapshot {
    let mut n = Node::new(
        name,
        NodeSpec {
            ring: 1,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: total,
        },
    );
    n.status = Some(NodeStatus {
        ready: true,
        last_heartbeat: Some("2026-07-04T00:00:00Z".to_owned()),
        allocatable: free,
        ..NodeStatus::default()
    });
    NodeSnapshot::from_node(&n)
}

fn slots(free: u32) -> Capacity {
    Capacity {
        max_workloads: free,
        ..Capacity::default()
    }
}

#[test]
fn saturated_node_is_not_selected() {
    // Both ready; the saturated one (0 of 4 slots free) must drop out, and the
    // one with headroom takes the placement.
    let saturated = node("saturated", slots(4), slots(0));
    let roomy = node("roomy", slots(4), slots(3));
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&workload()),
        &[saturated, roomy],
    );
    assert_eq!(decision.scheduled_node(), Some("roomy"));
}

#[test]
fn less_loaded_node_is_preferred() {
    // Both have headroom; the placement reflects real load — the idler wins.
    let total = Capacity {
        cpu_millis: 1000,
        memory_mb: 1000,
        gpu: 0,
        max_workloads: 10,
    };
    let busy = node(
        "busy",
        total,
        Capacity {
            cpu_millis: 100,
            memory_mb: 100,
            gpu: 0,
            max_workloads: 1,
        },
    );
    let idle = node(
        "idle",
        total,
        Capacity {
            cpu_millis: 900,
            memory_mb: 900,
            gpu: 0,
            max_workloads: 9,
        },
    );
    let decision =
        attested_scheduler().schedule(&ScheduleRequest::from_workload(&workload()), &[busy, idle]);
    assert_eq!(decision.scheduled_node(), Some("idle"));
}
