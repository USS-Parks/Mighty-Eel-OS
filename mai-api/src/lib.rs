#![allow(unused_variables, dead_code, missing_docs)]
//! # MAI API Server
//!
//! REST (axum) + gRPC (tonic) + SSE/WebSocket streaming API server.
//! This is the syscall interface in the Tock trust model: the stable
//! contract that applications compile against. It never breaks backward
//! compatibility.
//!
//! ## Endpoints
//!
//! - `/v1/chat/completions` - Streaming and non-streaming chat
//! - `/v1/completions` - Text completion
//! - `/v1/embeddings` - Vector embedding
//! - `/v1/models` - Model listing and management
//! - `/v1/health` - System health
//! - `/v1/power` - Power state queries and transitions
//! - `/v1/admin/*` - Configuration and registry management
//!
//! ## Trust Model
//!
//! This crate is TRUSTED. It sits inside the Tock kernel boundary.
//! All external input is validated at this layer before reaching
//! mai-core. Backend adapter names are never exposed in responses.

#![forbid(unsafe_code)]

// Foundation + Middleware modules
pub mod air_gap;
pub mod audit;
pub mod auth;
pub mod config;
pub mod errors;
pub mod types;

// Production profile skeleton (parsing-only; runtime wiring added later)
pub mod ship_profile;

// Production readiness guard (config-only checks; runtime checks added later)
pub mod production_guard;

// Vault builder (selects ZfsVault / LocalDevStubVault by ship profile)
pub mod vault_builder;

// Persistent API audit WAL writer (replaces MemoryAuditWriter in production)
pub mod audit_wal;

// Sealer builder (selects AeadSealer / NullSealer for compliance audit WAL)
pub mod sealer_builder;

// Trust builder (selects bundle verifier, loads ML-DSA anchors, picks token-exchange mode)
pub mod trust_builder;

// REST API Endpoints
pub mod handlers;
pub mod routes;
pub mod state;

// Observability — metrics registry + request-path middleware.
pub mod metrics;
pub mod middleware;

// SCAN-1 (Security SEC-011-MAI): token-bucket rate-limit scaffold.
// Module compiles + has unit tests; wiring into the route stack is
// the SEC-95 follow-up. See `docs/SCAN-1-INTERNAL-GITDOCTOR-REPORT.md`.
pub mod rate_limit;

// SSE Streaming + WebSocket
pub mod streaming;

// gRPC Server
pub mod grpc;

// Server Bootstrap
pub mod server;

// OpenBao Trust Bridge HTTP client
pub mod openbao_client;

// Public re-exports for SDK consumers and binary entry point
pub use audit_wal::{
    ReplayOutcome, WalAuditConfig, WalAuditError, WalAuditWriter, replay_and_verify,
};
pub use config::ServerConfig;
pub use errors::ApiError;
pub use production_guard::{
    CheckSeverity, CheckStatus, ProductionCheck, ProductionReadinessReport, ReadinessCounts,
    RuntimeChecks, RuntimeOutcome,
};
pub use sealer_builder::{SealerBuildError, build_sealer, sealer_key_path};
pub use server::MaiServer;
pub use server::ServerError;
pub use ship_profile::{ShipProfile, ShipProfileError, load_ship_profile, parse_ship_profile};
pub use trust_builder::{
    TrustBuildError, TrustComponents, TrustExchangeMode, boot_bundle_path, build_trust_components,
    verify_boot_bundle,
};
pub use vault_builder::{VaultBuildError, build_vault};
