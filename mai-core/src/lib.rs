#![allow(unused_variables, dead_code, missing_docs)]
//! MAI Core Kernel - Trusted inference orchestration layer
//!
//! This crate implements the trusted center of the Model Abstraction Interface:
//! - Model scheduling across adapters and GPUs
//! - Model registry with air-gap-safe updates
//! - Health monitoring with local-only telemetry
//! - Power state machine for sleep mode management
//! - Hot-swap manager for zero-downtime updates
//!
//! # Trust Model
//!
//! All code in this crate is TRUSTED. It may use `unsafe` only when absolutely
//! necessary and must be audited. Prefer safe Rust patterns. All hardware access
//! goes through the `mai-hil` traits.
//!
//! # No Network Dependencies
//!
//! This crate must work in air-gapped mode. Do not add dependencies that initiate
//! network connections. Telemetry is local-only.

#![forbid(unsafe_code)] // Enforced by CI; drivers in mai-hil may use unsafe
// #![warn(missing_docs)] // Re-enable after stub phase (Session 08+)

pub mod health;
pub mod hotswap;
pub mod power;
pub mod registry;
pub mod scheduler;
pub mod vault; // L2 interface, implemented in Session 12

// Re-export core types for convenience
pub use health::{AlertLevel, HealthMonitor, HealthSnapshot};
pub use hotswap::{HotSwapManager, SwapRequest, SwapResult};
pub use power::{PowerState, PowerStateMachine, TransitionTrigger};
pub use registry::{ModelManifest, ModelRegistry, ModelStatus};
pub use scheduler::{InferenceRequest, RequestPriority, Scheduler, SchedulerConfig};

// Core error type
pub use errors::CoreError;

mod errors;
pub mod types;
