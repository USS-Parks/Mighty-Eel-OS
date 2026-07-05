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
//! for a node returns `None` to *abstain* — it never fabricates a value — while
//! hard exclusion stays a filter's job. The [`ReadinessFilter`] is the concrete
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
pub mod scorers;
pub mod types;

pub use filters::{AttestationFilter, CapacityFilter, ReadinessFilter, RingFilter};
pub use framework::{Filter, Scheduler, Scorer};
pub use scorers::UtilizationScorer;
pub use types::{
    FilterVerdict, NodeEvaluation, NodeSnapshot, ScheduleOutcome, ScheduleRequest,
    SchedulingDecision, SignalProvenance,
};

/// The S1 baseline wiring: the readiness foundation only, no capacity or
/// scorers. Kept as the minimal, stable base the framework's own tests pin to;
/// [`attested_scheduler`] is the wiring the control plane drives.
pub fn baseline_scheduler() -> Scheduler {
    Scheduler::new().with_filter(ReadinessFilter)
}

/// The current Phase-S wiring the binding controller (S7) drives: the hard
/// filters and soft scorers landed so far — readiness, ring (S3), attestation
/// (S4) and capacity filters plus the utilisation scorer. The budget/ROI (S5)
/// and spread/HA (S6) scorers join here as they land.
pub fn attested_scheduler() -> Scheduler {
    Scheduler::new()
        .with_filter(ReadinessFilter)
        .with_filter(RingFilter)
        .with_filter(AttestationFilter)
        .with_filter(CapacityFilter)
        .with_scorer(1.0, UtilizationScorer)
}
