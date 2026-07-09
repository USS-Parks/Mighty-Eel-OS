//! MAI operator tooling. Provides `backup create` / `backup
//! verify` and `restore plan` / `restore apply`. Library
//! surface area is kept narrow on purpose so the CLI binary and the
//! burn-in scripts can drive both directions without
//! re-spawning the process.

pub mod audit;
pub mod backup;
pub mod manifest;
pub mod profile;
pub mod restore;

pub use audit::{AuditEntry, GENESIS_HASH, verify_chain};
pub use backup::{
    BackupError, BackupOptions, BackupReport, VerifyReport, create_backup, verify_backup,
};
pub use manifest::{
    BackupManifest, MLDSA87_PK_LEN, MLDSA87_SIG_LEN, MLDSA87_SK_LEN, ManifestComponent,
    ManifestError, ManifestSignatures, VerifyOutcome, sha3_file, sha3_hex, sha3_tree,
};
pub use profile::{
    BackupSourceProfile, ProfileLoadError, load_backup_source_profile, parse_backup_source_profile,
};
pub use restore::{
    ActionKind, RestoreAction, RestoreError, RestoreObstacle, RestorePlan, RestoreReport,
    RestoreSignatureRecord, RestoredComponent, apply_restore, plan_restore,
};
