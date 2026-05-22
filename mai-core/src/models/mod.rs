//! # Model Package Management (Session 24)
//!
//! Implements the `.mai-pkg` directory format, USB discovery, signature
//! verification, installation pipeline, and secure removal. These modules
//! build on top of `ModelRegistry` for lifecycle tracking and the vault
//! traits for cryptographic operations and storage.

pub mod install;
pub mod package;
pub mod remove;
pub mod usb;
pub mod verify;

pub use install::{InstallProgress, install_package};
pub use package::ModelPackage;
pub use remove::{RemovalResult, RemoveOptions, remove_model};
pub use usb::{DiscoveryResult, discover_usb_packages, scan_path_for_packages};
pub use verify::{VerificationResult, verify_package};
