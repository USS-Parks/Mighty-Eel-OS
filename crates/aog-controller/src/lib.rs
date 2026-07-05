//! `aog-controller` — Loom's reconciliation runtime (Phase R).
//!
//! R1: the level-triggered controller framework — a dedup-ing [`WorkQueue`]
//! with per-key exponential backoff and delayed requeue, and a [`Controller`]
//! loop that observes desired state through the K4 informer and drives one
//! [`Reconciler`] over the keys that changed, gated so only the leading
//! replica acts ([`LeaderGate`]). The Phase-R controllers (Tenant, TrustRing,
//! Capability, PolicyBundle, …) are reconcilers run on this runtime.
//!
//! R6: the [`PolicyBundleController`] signs each `PolicyBundle` and publishes it
//! to the channel gateway/node edges poll ([`BundleStore`]); an edge verifies
//! it with the control-plane public key alone and refuses a stale replay
//! ([`EdgeBundleCache`]).
//!
//! R7: the [`ProviderPoolController`] folds live provider/model health
//! ([`HealthProbe`]) into each pool's schedulable set, so the scheduler only
//! places on reachable endpoints.
//!
//! R8: the [`VirtualKeyController`] resolves each `VirtualKey` to a token minted
//! from its `Capability` and writes it to the gateway's key-resolution path, so
//! a key change is reflected at the gateway (G1) without a restart.
//!
//! R9: the [`RevocationController`] fans each `RevocationIntent` out to a signed
//! revocation snapshot — online (the gateway kill-switch path) and on removable
//! media (air-gap) — so a revoked token is denied on every replica and offline.
//!
//! X2: the [`WorkloadController`] brings `aog-gateway` under Loom as a managed
//! `Workload` — reconciling its health/readiness and reflecting its placements —
//! with no change to the gateway's data-path API.
//!
//! O1: the replica-set [`deploy`] planner makes the binding controller the full
//! Deployment analog — replica-indexed placements converge a `Workload` to
//! exactly its declared `replicas`, packing onto node capacity when replicas
//! outnumber nodes and dropping the excess (revoking their tokens) on scale-down.
//!
//! Trust posture: this crate's read path is the informer (bounded-stale,
//! resync-recovered, A1.6); its write path is **never** the store directly —
//! a controller mutates desired state only through the apiserver admission
//! chain (`aog-apiserver`), so every controller action is validated, sealed,
//! and receipted like any other caller's (A1.7, doctrine I-3/I-5).

pub mod bundle_store;
pub mod bundles;
pub mod capability;
pub mod deploy;
pub mod gc;
pub mod health;
pub mod intents;
pub mod node;
pub mod objects;
pub mod providers;
pub mod provision;
pub mod queue;
pub mod revocation;
pub mod rings;
pub mod runtime;
pub mod scheduler;
pub mod teardown;
pub mod transit;
pub mod vkeys;
pub mod workloads;

pub use bundle_store::{
    BundleReject, BundleStore, EdgeBundleCache, MemBundleStore, OpenBaoBundleStore, SignedBundle,
    sign_bundle, verify_bundle,
};
pub use bundles::PolicyBundleController;
pub use capability::CapabilityController;
pub use deploy::{ReplicaPlan, placement_name, plan_replicas, replica_index};
pub use gc::GarbageCollector;
pub use health::{HealthProbe, HttpHealthProbe};
pub use intents::RevocationIndexer;
pub use node::NodeController;
pub use objects::{EstateClient, is_terminating, parse_key};
pub use providers::ProviderPoolController;
pub use provision::{OPENBAO_FINALIZER, TenantProvisioner};
pub use queue::{Backoff, WorkQueue};
pub use revocation::RevocationController;
pub use rings::TrustRingController;
pub use runtime::{
    Action, AlwaysLeader, Controller, LeaderGate, ReconcileError, Reconciler, SharedGate, SyncStats,
};
pub use scheduler::SchedulerController;
pub use teardown::{TENANT_FINALIZER, TenantTeardown};
pub use vkeys::{VIRTUALKEY_FINALIZER, VirtualKeyController};
pub use workloads::{HttpWorkloadProbe, StaticWorkloadProbe, WorkloadController, WorkloadProbe};
