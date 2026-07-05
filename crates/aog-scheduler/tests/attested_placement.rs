//! Phase-S attested-placement integration tests, exercised through
//! `attested_scheduler()` — the wiring the control plane drives. Grows across
//! S2 (capacity + utilisation), S3 (ring), S4 (attestation).

use aog_estate::{
    AttestationPlatform, AttestationProfile, Capacity, Node, NodeSpec, NodeStatus, Workload,
    WorkloadKind, WorkloadSpec,
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

/// A ready node in a given ring with ample capacity.
fn ring_node(name: &str, ring: u8) -> NodeSnapshot {
    let mut n = Node::new(
        name,
        NodeSpec {
            ring,
            attestation_floor: Classification::Secret,
            attestation: AttestationProfile::default(),
            capacity: slots(4),
        },
    );
    n.status = Some(NodeStatus {
        ready: true,
        last_heartbeat: Some("2026-07-04T00:00:00Z".to_owned()),
        allocatable: slots(4),
        ..NodeStatus::default()
    });
    NodeSnapshot::from_node(&n)
}

#[test]
fn cross_ring_placement_is_impossible() {
    // The only node is ring 1; a ring-2 workload cannot be placed on it.
    let mut wl = workload();
    wl.spec.ring = 2;
    let decision =
        attested_scheduler().schedule(&ScheduleRequest::from_workload(&wl), &[ring_node("r1", 1)]);
    assert!(decision.is_pending());
}

#[test]
fn same_ring_node_takes_the_workload() {
    let mut wl = workload();
    wl.spec.ring = 2;
    let decision =
        attested_scheduler().schedule(&ScheduleRequest::from_workload(&wl), &[ring_node("r2", 2)]);
    assert_eq!(decision.scheduled_node(), Some("r2"));
}

/// A ready node with an explicit attestation profile and ample capacity.
fn attested_node(
    name: &str,
    ring: u8,
    floor: Classification,
    platform: AttestationPlatform,
    pcr: bool,
) -> NodeSnapshot {
    let mut n = Node::new(
        name,
        NodeSpec {
            ring,
            attestation_floor: floor,
            attestation: AttestationProfile {
                platform,
                air_gapped: true,
                pcr: pcr.then(|| "pcr-digest".to_owned()),
            },
            capacity: slots(4),
        },
    );
    n.status = Some(NodeStatus {
        ready: true,
        last_heartbeat: Some("2026-07-04T00:00:00Z".to_owned()),
        allocatable: slots(4),
        ..NodeStatus::default()
    });
    NodeSnapshot::from_node(&n)
}

fn ring3_secret_workload() -> Workload {
    let mut wl = workload();
    wl.spec.ring = 3;
    wl.spec.classification_ceiling = Classification::Secret;
    wl
}

#[test]
fn ring3_secret_refused_on_underattested_node() {
    // The only node claims a Secret floor but has no hardware root — it is
    // under-attested. The Ring-3 Secret workload must stay Pending, never
    // force-placed to relieve pressure (the S4 differentiator).
    let underattested = attested_node(
        "bare",
        3,
        Classification::Secret,
        AttestationPlatform::None,
        false,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret_workload()),
        &[underattested],
    );
    assert!(decision.is_pending());
}

#[test]
fn ring3_secret_placed_on_attested_node() {
    let attested = attested_node(
        "tpm",
        3,
        Classification::Secret,
        AttestationPlatform::Tpm,
        true,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret_workload()),
        &[attested],
    );
    assert_eq!(decision.scheduled_node(), Some("tpm"));
}

#[test]
fn never_force_placed_on_least_bad_node() {
    // Two nodes, both unfit for a Ring-3 Secret workload: one with too low a
    // floor, one with a high floor but no hardware root. Neither is chosen.
    let low_floor = attested_node(
        "low",
        3,
        Classification::Internal,
        AttestationPlatform::Tpm,
        true,
    );
    let no_hardware = attested_node(
        "nohw",
        3,
        Classification::Secret,
        AttestationPlatform::None,
        false,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret_workload()),
        &[low_floor, no_hardware],
    );
    assert!(decision.is_pending());
}
