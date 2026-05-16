#![allow(unused_variables)]
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
#![warn(missing_docs)]

pub mod scheduler;
pub mod registry;
pub mod health;
pub mod power;
pub mod hotswap;
pub mod vault; // L2 interface, implemented in Session 12

// Re-export core types for convenience
pub use scheduler::{Scheduler, SchedulerConfig, InferenceRequest, RequestPriority};
pub use registry::{ModelRegistry, ModelManifest, ModelStatus};
pub use health::{HealthMonitor, HealthSnapshot, AlertLevel};
pub use power::{PowerStateMachine, PowerState, TransitionTrigger};
pub use hotswap::{HotSwapManager, SwapRequest, SwapResult};

// Core error type
pub use errors::CoreError;

mod errors;
pub mod types;
