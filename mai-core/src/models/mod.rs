//! # Model Package Management
//!
//! Implements the `.mai-pkg` directory format, USB discovery, signature
//! verification, installation pipeline, and secure removal. These modules
//! build on top of `ModelRegistry` for lifecycle tracking and the vault
//! traits for cryptographic operations and storage.

pub mod install;
pub mod lifecycle;
pub mod package;
pub mod preload;
pub mod remove;
pub mod update;
pub mod usb;
pub mod verify;

pub use lifecycle::{BenchmarkResult, DeploymentExport, InstalledModel, ModelLifecycleManager};
pub use package::ModelPackage;
pub use preload::{PreloadConfig, PreloadReason, PreloadTarget, build_preload_plan};
pub use update::{
    DifferentialPlan, LicenseEntitlement, UpdateCheckResult, UpdateClient, UpdateClientConfig,
    UpdateManifest, UpdateModel, UpdateRequest, UpdateResponse, UpdateTier, UpdateTransport,
    WeightShard, compare_manifest, plan_differential_download, seasonal_bundle, validate_license,
};
pub use usb::{DiscoveryResult, discover_usb_packages, scan_path_for_packages};
pub use verify::{VerificationResult, verify_package};
