//! V6 — attested-scheduling breach (addendum S4 / the A1.8 differentiator). The hard
//! rule: a Ring-3 classified workload is **never** placed on an under-attested node —
//! not to relieve pressure, not onto the "least bad" node, not to fill a ring. Every
//! `schedule()`-reachable avenue is attempted here; all must leave the workload
//! `Pending`.
//!
//! Two avenues are structurally excluded rather than tested by placement: **preemption**
//! (S8) only ever considers nodes that already passed the filter phase, so an
//! under-attested node — filtered out by `AttestationFilter` — is never a preemption
//! candidate; and a **race** between concurrent schedulers cannot produce an
//! under-attested placement because both apply the same hard filter and the binding
//! write is CAS-guarded. What remains — pressure, a fleet of unfit flavors, and an
//! out-of-ring attested node — is exercised directly below.

use aog_estate::{
    AttestationPlatform, AttestationProfile, Capacity, Node, NodeSpec, NodeStatus, Workload,
    WorkloadKind, WorkloadSpec,
};
use aog_scheduler::{NodeSnapshot, ScheduleRequest, attested_scheduler};
use fabric_contracts::Classification;

fn slots(free: u32) -> Capacity {
    Capacity {
        max_workloads: free,
        ..Capacity::default()
    }
}

/// A ready node: ring, attestation floor, platform (+ optional PCR), and free slots.
fn node(
    name: &str,
    ring: u8,
    floor: Classification,
    platform: AttestationPlatform,
    pcr: bool,
    free: u32,
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
        last_heartbeat: Some("2026-07-06T00:00:00Z".to_owned()),
        allocatable: slots(free),
        ..NodeStatus::default()
    });
    NodeSnapshot::from_node(&n)
}

fn ring3_secret() -> Workload {
    Workload::new(
        "classified",
        WorkloadSpec {
            workload_kind: WorkloadKind::Gateway,
            replicas: 1,
            ring: 3,
            classification_ceiling: Classification::Secret,
            image: None,
            command: Vec::new(),
            capability: None,
        },
    )
}

/// Avenue — **pressure**: the one properly attested node is saturated (0 free slots)
/// while an under-attested node has ample room. The scheduler must NOT relieve pressure
/// by force-placing the Ring-3 workload onto the roomy-but-under-attested node.
#[test]
fn pressure_never_forces_onto_underattested() {
    let attested_full = node(
        "tpm-full",
        3,
        Classification::Secret,
        AttestationPlatform::Tpm,
        true,
        0,
    );
    let bare_roomy = node(
        "bare-roomy",
        3,
        Classification::Secret,
        AttestationPlatform::None,
        false,
        4,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret()),
        &[attested_full, bare_roomy],
    );
    assert!(
        decision.is_pending(),
        "pressure must not force a Ring-3 Secret workload onto an under-attested node: {decision:?}"
    );
}

/// Avenue — **fleet of unfit flavors**: every node is roomy but unfit for a different
/// reason (floor too low, no hardware root, TPM but no measurement, wide open). None
/// may be chosen; the workload stays Pending.
#[test]
fn no_underattested_flavor_is_ever_chosen() {
    let nodes = vec![
        node(
            "low-floor",
            3,
            Classification::Internal,
            AttestationPlatform::Tpm,
            true,
            4,
        ),
        node(
            "no-hw",
            3,
            Classification::Secret,
            AttestationPlatform::None,
            false,
            4,
        ),
        node(
            "no-pcr",
            3,
            Classification::Secret,
            AttestationPlatform::Tpm,
            false,
            4,
        ),
        node(
            "public",
            3,
            Classification::Public,
            AttestationPlatform::None,
            false,
            4,
        ),
    ];
    let decision =
        attested_scheduler().schedule(&ScheduleRequest::from_workload(&ring3_secret()), &nodes);
    assert!(
        decision.is_pending(),
        "no under-attested flavor may host a Ring-3 Secret workload: {decision:?}"
    );
}

/// Avenue — **fill the ring**: the only attested node is in the wrong ring (dropped by
/// the ring filter), leaving only roomy under-attested ring-3 nodes. The scheduler does
/// not relax attestation to fill ring 3.
#[test]
fn wrong_ring_attested_plus_roomy_underattested_stays_pending() {
    let attested_wrong_ring = node(
        "tpm-r2",
        2,
        Classification::Secret,
        AttestationPlatform::Tpm,
        true,
        4,
    );
    let bare_r3 = node(
        "bare-r3",
        3,
        Classification::Secret,
        AttestationPlatform::None,
        false,
        4,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret()),
        &[attested_wrong_ring, bare_r3],
    );
    assert!(decision.is_pending());
}

/// Control — a properly attested, in-ring, roomy node DOES take it, proving the refusals
/// above are the attestation predicate at work, not a scheduler that never places.
#[test]
fn a_properly_attested_node_still_takes_it() {
    let good = node(
        "tpm-good",
        3,
        Classification::Secret,
        AttestationPlatform::Tpm,
        true,
        4,
    );
    let bare = node(
        "bare",
        3,
        Classification::Secret,
        AttestationPlatform::None,
        false,
        4,
    );
    let decision = attested_scheduler().schedule(
        &ScheduleRequest::from_workload(&ring3_secret()),
        &[good, bare],
    );
    assert_eq!(decision.scheduled_node(), Some("tpm-good"));
}
