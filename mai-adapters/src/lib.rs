//! # MAI Adapter Framework
//!
//! The Rust-side adapter management framework. Spawns, monitors, and
//! communicates with Python adapter processes through a JSON-RPC IPC bridge.
//!
//! ## Trust Level: TRUSTED (framework) / UNTRUSTED (adapter processes)
//!
//! The `AdapterManager` runs in trusted Rust code. The Python adapter
//! processes it spawns are untrusted, isolated by cgroups, and
//! crash-recovered with exponential backoff.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │       AdapterManager (Rust)         │ ← Trusted
//! │  ┌─────────┐  ┌─────────────────┐  │
//! │  │HealthMon│  │  AuditLogger    │  │
//! │  └─────────┘  └─────────────────┘  │
//! └──────────┬──────────────────────────┘
//!            │ JSON-RPC over stdin/stdout
//! ┌──────────▼──────────────────────────┐
//! │    AdapterProcess (subprocess)       │ ← Untrusted
//! │    Python adapter runner + adapter   │
//! │    cgroups isolation, crash boundary │
//! └─────────────────────────────────────┘
//! ```

pub mod audit;
pub mod bridge;
pub mod config;
pub mod errors;
pub mod health;
pub mod manager;
pub mod process;
pub mod python_embed;
pub mod validation;

pub use errors::FrameworkError;
pub use manager::AdapterManager;
pub use process::{AdapterProcess, ProcessState};
pub use python_embed::{PythonRuntimeInfo, python_runtime_info};
pub use validation::{HostValidationError, validate_adapter_host};
