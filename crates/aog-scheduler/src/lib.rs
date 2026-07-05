//! `aog-scheduler` — Loom's attested placement engine (Phase S).
//!
//! Loom places AOG [`Workload`](aog_estate::Workload)s onto estate
//! [`Node`](aog_estate::Node)s. This crate is the decision engine that chooses
//! the node: a K8s-style **filter → score → bind** framework (A1.8), revived
//! from `mai-scheduler`'s `PlacementEngine`/`Scheduler` shape and rebuilt for
//! the AOG domain.
//!
//! # What "revived" deletes (S1)
//!
//! `mai-scheduler` routes inference requests to GPU instances; its metrics come
//! from real request feedback, but it carries one defect this revival refuses
//! to inherit: **absence-as-optimism**. An instance with zero telemetry scored
//! as maximally healthy (`metrics/health.rs`: an empty tracker returns `1.0`).
//! For inference routing that is a defensible cold-start guess. For *attested
//! placement* it is a custody breach: an unmeasured — therefore untrusted —
//! node would look fit, and a classified workload could land on it. Doctrine
//! I-4 is the opposite: every uncertainty resolves toward *less* privilege.
//!
//! So this crate rests on one rule: **every signal traces to a real
//! [`Node`](aog_estate::Node); absence is fail-closed, never fabricated.** A
//! [`NodeSnapshot`] is a verbatim projection of a node's reconciled state — a
//! node that has not reported has `ready == false` and zero allocatable
//! capacity, honestly, and is filtered out. A [`Scorer`] with no real signal
//! for a node returns `None`, which *excludes* the node; the framework never
//! fills the gap with a default. The [`ReadinessFilter`] is the concrete
//! inversion of the deleted defect.
//!
//! # The framework grows across Phase S
//!
//! S1 ships the framework and the readiness foundation. Later prompts register
//! more plugins on the same two seams: node capacity from real heartbeats (S2),
//! the hard ring filter (S3), the attestation predicate
//! `classification_ceiling <= attestation_floor` (S4), the budget/ROI (S5) and
//! spread/HA (S6) scorers, binding plus runtime-token mint (S7), and preemption
//! (S8). Writing the [`Placement`](aog_estate::Placement) and minting the
//! runtime token is S7; S1 selects the node and records why.

pub mod filters;
pub mod framework;
pub mod types;

pub use filters::ReadinessFilter;
pub use framework::{Filter, Scheduler, Scorer};
pub use types::{
    FilterVerdict, NodeEvaluation, NodeSnapshot, ScheduleOutcome, ScheduleRequest,
    SchedulingDecision, SignalProvenance,
};

/// The S1 baseline wiring: the readiness foundation, no scorers. Later Phase-S
/// prompts extend it with the ring (S3) and attestation (S4) filters and the
/// budget/ROI (S5) and spread/HA (S6) scorers.
pub fn baseline_scheduler() -> Scheduler {
    Scheduler::new().with_filter(ReadinessFilter)
}
