//! # MAI Core Kernel
//!
//! The trusted core of the Model Abstraction Interface. Contains the model
//! scheduler, model registry, power state machine, health monitor, hot-swap
//! manager, and vault interface.
//!
//! ## Trust Level: TRUSTED
//!
//! This crate contains zero `unsafe` code. All unsafe operations are confined
//! to `mai-hil` driver implementations. This invariant is enforced by CI.
//!
//! ## Modules
//!
//! - `scheduler`: Routes inference requests to adapters based on capability,
//!   load, and scheduling strategy.
//! - `registry`: Tracks model manifests, lifecycle state, and version history.
//! - `power`: Controls power state transitions (`DeepVaultSleep`, Sentinel,
//!   `FullInference`, `ThermalThrottle`) with auto-demotion timers.
//! - `health`: Monitors adapter heartbeats, hardware telemetry, and system
//!   integrity. Telemetry is local-only, never transmitted.
//! - `hotswap`: Zero-downtime model and adapter replacement with rollback.
//! - `vault`: Interface to L2 vault operations (ZFS, PQC encryption, profiles,
//!   audit trail, vector DB).

pub mod health;
pub mod hotswap;
pub mod power;
pub mod registry;
pub mod scheduler;
pub mod vault;
