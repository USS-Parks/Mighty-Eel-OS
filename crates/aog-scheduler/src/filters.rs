//! Filter plugins: hard filters that keep a node out of scheduling unless it has
//! actually reported. The readiness filter is the foundation; the ring filter and
//! the attestation predicate live here too.

use crate::framework::Filter;
use crate::types::{FilterVerdict, NodeSnapshot, ScheduleRequest};

/// Hard filter: a node is a candidate only when it has actually reported —
/// `status.ready` is true and a heartbeat is present.
///
/// A zero-telemetry instance must never be scored as maximally healthy; here a
/// node with no reconciled liveness is `Unfit`, never assumed live (doctrine I-4).
/// Because a
/// [`NodeSnapshot`] projects a status-less node to `ready == false`, an
/// unmeasured node fails this filter by construction — there is no path by which
/// the absence of a signal becomes a favourable one.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadinessFilter;

impl Filter for ReadinessFilter {
    fn name(&self) -> &'static str {
        "readiness"
    }

    fn filter(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        match (node.ready, node.last_heartbeat.is_some()) {
            (true, true) => FilterVerdict::Fit,
            (false, _) => FilterVerdict::unfit(
                "readiness",
                "node status.ready is false (no reconciled liveness)",
            ),
            (true, false) => {
                FilterVerdict::unfit("readiness", "node has never reported a heartbeat")
            }
        }
    }
}

/// Hard filter: a node with a declared workload-slot capacity but none free is
/// saturated and rejected (S2). Free slots come from the node's reconciled
/// `allocatable` — real reported headroom — so a saturated node drops out of
/// candidacy rather than being packed further. A node that declares no slot
/// budget is not constrained on slots here (the utilisation scorer still weighs
/// its cpu/memory/gpu load).
#[derive(Debug, Clone, Copy, Default)]
pub struct CapacityFilter;

impl Filter for CapacityFilter {
    fn name(&self) -> &'static str {
        "capacity"
    }

    fn filter(&self, _request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        if node.capacity.max_workloads > 0 && node.allocatable.max_workloads == 0 {
            return FilterVerdict::unfit(
                "capacity",
                "node is at workload capacity (0 free of declared slots)",
            );
        }
        FilterVerdict::Fit
    }
}

/// Hard filter: a workload is placed only within its own trust ring (S3). Rings
/// are the Trust Manifold's isolation boundary; crossing one is a sovereignty
/// violation, so a ring mismatch is `Unfit`, full stop — no score can rescue it.
#[derive(Debug, Clone, Copy, Default)]
pub struct RingFilter;

impl Filter for RingFilter {
    fn name(&self) -> &'static str {
        "ring"
    }

    fn filter(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        if request.ring == node.ring {
            FilterVerdict::Fit
        } else {
            FilterVerdict::unfit(
                "ring",
                format!(
                    "workload ring {} does not match node ring {}",
                    request.ring, node.ring
                ),
            )
        }
    }
}

/// Hard filter — the attested-placement guard (A1.3.2). A workload is placed
/// only where its data-classification ceiling is within the node's attestation
/// floor (`classification_ceiling <= node.attestation_floor`), and any sensitive
/// workload (ceiling `>= Restricted`) additionally requires the node to declare
/// a hardware attestation platform (TPM / Nitro / SEV-SNP) with a recorded PCR.
/// A bare high-floor claim with no platform/PCR is refused and the workload
/// stays Pending rather than force-placed to relieve pressure (doctrine I-2/I-4).
///
/// LIMITATION (2026-07-08 audit): the `platform` + `pcr` are the
/// node's **self-declared** values — this filter checks their *presence*, it does
/// not yet verify a control-plane-checked hardware quote (signed quote + AK cert
/// chain + pinned reference PCRs + fresh nonce). That verification needs real
/// TPM/attestation hardware and is deferred to the hardware lane; until it lands
/// a node that asserts a platform + PCR is trusted for Restricted+ placement.
/// Fully closing this gap is either that verification or an owner decision to deny
/// Restricted+ fail-closed (a placement-capability change, so owner-gated).
#[derive(Debug, Clone, Copy, Default)]
pub struct AttestationFilter;

impl Filter for AttestationFilter {
    fn name(&self) -> &'static str {
        "attestation"
    }

    fn filter(&self, request: &ScheduleRequest, node: &NodeSnapshot) -> FilterVerdict {
        use aog_estate::AttestationPlatform;
        use fabric_contracts::Classification;

        let ceiling = request.classification_ceiling;
        let floor = node.attestation_floor;
        if ceiling > floor {
            return FilterVerdict::unfit(
                "attestation",
                format!(
                    "workload classification ceiling {ceiling:?} exceeds node attestation floor {floor:?}"
                ),
            );
        }
        // A sensitive ceiling demands a hardware-rooted floor: a bare assertion
        // is not attestation (I-4). Public / Internal need no hardware root.
        if ceiling >= Classification::Restricted {
            // TODO(basho): the platform + pcr below are node-self-declared
            // (2026-07-08 audit). Verify a control-plane-checked
            // hardware quote (AK cert chain + pinned PCRs + fresh nonce) before
            // trusting them; blocked on TPM/attestation hardware.
            if node.attestation.platform == AttestationPlatform::None {
                return FilterVerdict::unfit(
                    "attestation",
                    format!(
                        "classification {ceiling:?} requires a hardware attestation platform; node reports none"
                    ),
                );
            }
            if node.attestation.pcr.is_none() {
                return FilterVerdict::unfit(
                    "attestation",
                    format!(
                        "classification {ceiling:?} requires a recorded PCR measurement; node reports none"
                    ),
                );
            }
        }
        FilterVerdict::Fit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aog_estate::{AttestationPlatform, AttestationProfile, Capacity, WorkloadKind};
    use fabric_contracts::Classification;

    fn snap(ready: bool, heartbeat: bool) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready,
            capacity: Capacity::default(),
            allocatable: Capacity::default(),
            last_heartbeat: heartbeat.then(|| "t".to_owned()),
            resource_version: 1,
        }
    }

    fn cap_snap(total_slots: u32, free_slots: u32) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 1,
            attestation_floor: Classification::Public,
            attestation: AttestationProfile::default(),
            ready: true,
            capacity: Capacity {
                max_workloads: total_slots,
                ..Capacity::default()
            },
            allocatable: Capacity {
                max_workloads: free_slots,
                ..Capacity::default()
            },
            last_heartbeat: Some("t".to_owned()),
            resource_version: 1,
        }
    }

    fn req() -> ScheduleRequest {
        ScheduleRequest {
            workload_name: "wl".to_owned(),
            workload_kind: WorkloadKind::Gateway,
            ring: 1,
            classification_ceiling: Classification::Public,
            already_placed_on: Vec::new(),
        }
    }

    #[test]
    fn ready_with_heartbeat_is_fit() {
        assert!(ReadinessFilter.filter(&req(), &snap(true, true)).is_fit());
    }

    #[test]
    fn not_ready_is_unfit() {
        assert!(!ReadinessFilter.filter(&req(), &snap(false, true)).is_fit());
    }

    #[test]
    fn ready_without_heartbeat_is_unfit() {
        // The defect inversion: a `ready` flag with no heartbeat is still unfit.
        assert!(!ReadinessFilter.filter(&req(), &snap(true, false)).is_fit());
    }

    #[test]
    fn saturated_node_is_unfit() {
        assert!(!CapacityFilter.filter(&req(), &cap_snap(8, 0)).is_fit());
    }

    #[test]
    fn node_with_free_slots_is_fit() {
        assert!(CapacityFilter.filter(&req(), &cap_snap(8, 3)).is_fit());
    }

    #[test]
    fn undeclared_slot_capacity_is_not_filtered() {
        // A node that declares no slot budget is not rejected on slots.
        assert!(CapacityFilter.filter(&req(), &cap_snap(0, 0)).is_fit());
    }

    #[test]
    fn matching_ring_is_fit() {
        let mut node = snap(true, true);
        node.ring = 2;
        let mut request = req();
        request.ring = 2;
        assert!(RingFilter.filter(&request, &node).is_fit());
    }

    #[test]
    fn ring_mismatch_is_unfit() {
        let node = snap(true, true); // ring 1
        let mut request = req();
        request.ring = 3;
        assert!(!RingFilter.filter(&request, &node).is_fit());
    }

    fn att_snap(floor: Classification, platform: AttestationPlatform, pcr: bool) -> NodeSnapshot {
        NodeSnapshot {
            name: "n".to_owned(),
            ring: 3,
            attestation_floor: floor,
            attestation: AttestationProfile {
                platform,
                air_gapped: true,
                pcr: pcr.then(|| "pcr-digest".to_owned()),
            },
            ready: true,
            capacity: Capacity::default(),
            allocatable: Capacity::default(),
            last_heartbeat: Some("t".to_owned()),
            resource_version: 1,
        }
    }

    fn req_ceiling(ceiling: Classification) -> ScheduleRequest {
        ScheduleRequest {
            workload_name: "wl".to_owned(),
            workload_kind: WorkloadKind::Gateway,
            ring: 3,
            classification_ceiling: ceiling,
            already_placed_on: Vec::new(),
        }
    }

    #[test]
    fn ceiling_within_backed_floor_is_fit() {
        let node = att_snap(Classification::Secret, AttestationPlatform::Tpm, true);
        assert!(
            AttestationFilter
                .filter(&req_ceiling(Classification::Secret), &node)
                .is_fit()
        );
    }

    #[test]
    fn ceiling_above_floor_is_unfit() {
        let node = att_snap(Classification::Internal, AttestationPlatform::Tpm, true);
        assert!(
            !AttestationFilter
                .filter(&req_ceiling(Classification::Secret), &node)
                .is_fit()
        );
    }

    #[test]
    fn sensitive_workload_needs_hardware_platform() {
        // Floor claims Secret but there is no hardware root → under-attested.
        let node = att_snap(Classification::Secret, AttestationPlatform::None, false);
        assert!(
            !AttestationFilter
                .filter(&req_ceiling(Classification::Secret), &node)
                .is_fit()
        );
    }

    #[test]
    fn sensitive_workload_needs_pcr() {
        let node = att_snap(Classification::Secret, AttestationPlatform::Tpm, false);
        assert!(
            !AttestationFilter
                .filter(&req_ceiling(Classification::Restricted), &node)
                .is_fit()
        );
    }

    #[test]
    fn public_workload_needs_no_hardware() {
        let node = att_snap(Classification::Public, AttestationPlatform::None, false);
        assert!(
            AttestationFilter
                .filter(&req_ceiling(Classification::Public), &node)
                .is_fit()
        );
    }
}
