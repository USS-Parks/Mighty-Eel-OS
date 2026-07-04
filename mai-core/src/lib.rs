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

pub mod airgap; // canonical ConnectivityState shared across crates
pub mod cache;
pub mod circuit_breaker;
pub mod health;
pub mod hotswap;
pub mod models;
pub mod power;
pub mod registry;
pub mod scheduler;
pub mod sentinel;
pub mod vault; // L2 interface

// Re-export core types for convenience
pub use airgap::{AirGapPolicy, ConnectivityState};
pub use cache::ResponseCache;
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
pub use health::{AlertLevel, HealthMonitor, HealthSnapshot};
pub use hotswap::{HotSwapManager, SwapRequest, SwapResult};
pub use power::{PowerState, PowerStateMachine, TransitionTrigger};
pub use registry::{ModelManifest, ModelRegistry, ModelStatus};
pub use scheduler::{InferenceRequest, RequestPriority, Scheduler, SchedulerConfig};
pub use sentinel::{
    Complexity, ProductTier, PromoteReason, PromotionFlow, PromotionFlowState, PromotionState,
    RequestComplexityEstimator, RequestFeatures, SentinelConfig, SentinelRuntime, TaskKind,
    WarmupDecider, WarmupStrategy,
};
// L2 vault types and traits
pub use vault::{AuditStore, ModelStorage, PqcProvider, ProfileStore, TpmProvider, VectorStore};
pub use vault::{
    CollectionConfig, ComplianceReport, DistanceMetric, EmbeddingPoint, FamilyProfile, FullVault,
    IntegrityResult, KeyInfo, KeyLevel, ProfileChangeEvent, ProfilePermissions, ProfileRole,
    SearchResult, SnapshotInfo, StorageInfo, VaultAuditAction, VaultAuditEntry, VaultAuditStatus,
    VaultError, VaultInterface,
};

// Core error type
pub use errors::CoreError;

mod errors;
pub mod types;
