//! # MAI Adapter Framework
//!
//! The Rust-side adapter management framework. Spawns, monitors, and
//! communicates with Python adapter processes through the `PyO3` FFI bridge.
//!
//! ## Trust Level: TRUSTED (framework) / UNTRUSTED (adapter processes)
//!
//! The `AdapterManager` runs in trusted Rust code. The Python adapter
//! processes it spawns are untrusted, isolated by cgroups, and
//! crash-recoverable.

// Stub: implementation in Session 08
