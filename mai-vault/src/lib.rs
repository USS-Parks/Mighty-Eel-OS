//! MAI Vault Implementation (L2)
//!
//! Concrete implementations of the vault traits defined in `mai-core::vault`.
//! This crate provides:
//!
//! - ZFS encrypted model storage with snapshot management
//! - Post-quantum cryptography (ML-KEM-1024 + ML-DSA-87)
//! - TPM 2.0 hardware key sealing
//! - SQLite-backed family profile store
//! - Hash-chained, PQC-signed audit trail
//! - Local Qdrant vector database interface
//!
//! # Air-Gap Safety
//!
//! This crate must never initiate outbound network connections.
//! The Qdrant instance is local-only (127.0.0.1:6334).

#![forbid(unsafe_code)]
#![allow(unused_variables, dead_code, missing_docs)]

pub mod audit;
pub mod config;
pub mod file_dev;
pub mod init;
pub mod pqc;
pub mod profiles;
pub mod tpm;
pub mod vectors;
pub mod zfs;
pub mod zfs_ops;

pub use config::VaultConfig;
pub use file_dev::FileDevVault;
pub use zfs::ZfsVault;
pub use zfs_ops::{DatasetExpectations, ZfsOps};
