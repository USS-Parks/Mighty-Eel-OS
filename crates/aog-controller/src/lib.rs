//! `aog-controller` — Loom's reconciliation runtime (Phase R).
//!
//! R1: the level-triggered controller framework — a dedup-ing [`WorkQueue`]
//! with per-key exponential backoff and delayed requeue, and a [`Controller`]
//! loop that observes desired state through the K4 informer and drives one
//! [`Reconciler`] over the keys that changed, gated so only the leading
//! replica acts ([`LeaderGate`]). The Phase-R controllers (Tenant, TrustRing,
//! Capability, PolicyBundle, …) are reconcilers run on this runtime.
//!
//! Trust posture: this crate's read path is the informer (bounded-stale,
//! resync-recovered, A1.6); its write path is **never** the store directly —
//! a controller mutates desired state only through the apiserver admission
//! chain (`aog-apiserver`), so every controller action is validated, sealed,
//! and receipted like any other caller's (A1.7, doctrine I-3/I-5).

pub mod gc;
pub mod intents;
pub mod objects;
pub mod queue;
pub mod runtime;
pub mod teardown;

pub use gc::GarbageCollector;
pub use intents::RevocationIndexer;
pub use objects::{EstateClient, is_terminating, parse_key};
pub use queue::{Backoff, WorkQueue};
pub use runtime::{
    Action, AlwaysLeader, Controller, LeaderGate, ReconcileError, Reconciler, SharedGate, SyncStats,
};
pub use teardown::{TENANT_FINALIZER, TenantTeardown};
