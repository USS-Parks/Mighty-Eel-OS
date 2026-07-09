//! Vault builder.
//!
//! Construct a [`VaultInterface`] implementation from a parsed
//! [`ShipProfile`]. This is the bridge between the typed
//! profile and the live [`mai_vault::ZfsVault`] that
//! `ModelRegistry` consumes.
//!
//! Scope:
//! - Helper function [`build_vault`].
//! - Production profile rejects every non-real vault path.
//! - Local-dev profile permits the inline [`LocalDevStubVault`]
//!   only when `vault.allow_stub=true`.
//! - Existence check on `vault.root` for production mode.
//!
//! Out of scope:
//! - Wiring into `MaiServer::run()`. That lands in the
//!   convergence step alongside the production guard's runtime
//!   checks (`PROD-VAULT-100..102`).
//! - First-boot key initialization. SHIP-HARDENING-PLAN §4 step 5
//!   tracks that as a follow-up.
//! - Retiring `server::StubVault`. Convergence removes it once
//!   the builder owns the boot path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use mai_core::vault::{VaultError, VaultInterface};
use mai_vault::audit::AuditWriter as VaultAuditWriter;
use mai_vault::pqc::PqcEngine;
use mai_vault::{FileDevVault, VaultConfig as MaiVaultConfig, ZfsOps, ZfsVault};

use crate::ship_profile::{ProfileMode, ShipProfile, VaultBackend};

/// Errors produced by [`build_vault`] when a profile cannot yield a
/// usable vault implementation for the requested mode.
#[derive(Debug, thiserror::Error)]
pub enum VaultBuildError {
    #[error("ship profile selects backend {backend:?} but production mode forbids non-real vaults")]
    StubInProduction { backend: VaultBackend },
    #[error("production profile sets vault.allow_stub=true; refused by the vault builder")]
    StubAllowedInProduction,
    #[error("local-dev profile selects stub vault but vault.allow_stub=false")]
    StubNotAllowed,
    #[error("vault.root must be a non-empty path")]
    EmptyRoot,
    #[error("vault.root {path:?} does not exist; create it before boot in production")]
    RootMissing { path: PathBuf },
    #[error("vault initialization failed: {0}")]
    InitFailed(String),
}

/// Construct the vault implementation selected by the profile.
///
/// Behavior matrix (V1: production accepts **only** the reviewed encrypted
/// backend — ZFS; `stub` is not a vault and `file-dev` stores plaintext):
///
/// | Mode        | Backend  | allow_stub | Outcome                       |
/// |-------------|----------|------------|-------------------------------|
/// | production  | zfs      | false      | [`ZfsVault`]                  |
/// | production  | zfs      | true       | [`StubAllowedInProduction`]   |
/// | production  | stub     | any        | [`StubInProduction`]          |
/// | production  | file-dev | any        | [`StubInProduction`]          |
/// | local-dev   | zfs      | any        | [`ZfsVault`]                  |
/// | local-dev   | stub     | true       | [`LocalDevStubVault`]         |
/// | local-dev   | stub     | false      | [`StubNotAllowed`]            |
/// | local-dev   | file-dev | any        | [`FileDevVault`]               |
///
/// [`StubAllowedInProduction`]: VaultBuildError::StubAllowedInProduction
/// [`StubInProduction`]: VaultBuildError::StubInProduction
/// [`FileDevVault`]: mai_vault::file_dev::FileDevVault
/// [`StubNotAllowed`]: VaultBuildError::StubNotAllowed
///
/// V2/V3: the ZFS arm returns an **initialized** vault — PQC and audit
/// engines constructed and initialized, the storage tree scanned, and (when
/// `vault.dataset` is set) the live dataset's properties proven (V5) — and
/// any initialization failure is an error, so a production boot never binds
/// sockets over an uninitialized vault.
pub async fn build_vault(
    profile: &ShipProfile,
) -> Result<Box<dyn VaultInterface>, VaultBuildError> {
    let root = profile.vault.root.as_path();
    if root.as_os_str().is_empty() {
        return Err(VaultBuildError::EmptyRoot);
    }

    let is_production = matches!(profile.profile.mode, ProfileMode::Production);

    if is_production && profile.vault.allow_stub {
        return Err(VaultBuildError::StubAllowedInProduction);
    }

    match profile.vault.backend {
        VaultBackend::Stub => {
            if is_production {
                Err(VaultBuildError::StubInProduction {
                    backend: VaultBackend::Stub,
                })
            } else if !profile.vault.allow_stub {
                Err(VaultBuildError::StubNotAllowed)
            } else {
                Ok(Box::new(LocalDevStubVault))
            }
        }
        VaultBackend::FileDev => {
            // V1: file-dev stores model material in plaintext — a development
            // convenience, never a production vault. Reject regardless of
            // allow_stub or root state.
            if is_production {
                return Err(VaultBuildError::StubInProduction {
                    backend: VaultBackend::FileDev,
                });
            }
            Ok(Box::new(FileDevVault::new(zfs_config_from_root(root))))
        }
        VaultBackend::Zfs => {
            if is_production && !root.exists() {
                return Err(VaultBuildError::RootMissing {
                    path: root.to_path_buf(),
                });
            }
            let mut cfg = zfs_config_from_root(root);
            if let Some(dataset) = &profile.vault.dataset {
                cfg.storage.dataset = dataset.clone();
            }

            // V2: engines are constructed and initialized here — the old
            // path handed out a bare `ZfsVault::new` with no PQC, no audit
            // writer, and nothing awaited.
            let pqc = Arc::new(PqcEngine::new(cfg.pqc.clone()));
            pqc.initialize()
                .await
                .map_err(|e| VaultBuildError::InitFailed(format!("pqc: {e}")))?;
            let audit = Arc::new(VaultAuditWriter::with_pqc(cfg.audit.clone(), pqc.clone()));
            audit
                .initialize()
                .await
                .map_err(|e| VaultBuildError::InitFailed(format!("audit: {e}")))?;

            // Storage tree must exist before the vault scans it.
            for dir in [&cfg.storage.mount_point, &cfg.storage.staging_dir] {
                std::fs::create_dir_all(dir)
                    .map_err(|e| VaultBuildError::InitFailed(format!("storage tree: {e}")))?;
            }

            // V5: when the profile names the backing dataset, wire real ZFS
            // ops so initialization proves the dataset (encryption, keys,
            // mountpoint) instead of trusting a directory.
            let mut vault = ZfsVault::with_engines(cfg, pqc, audit);
            if profile.vault.dataset.is_some() {
                vault = vault.with_zfs(ZfsOps::system());
            }

            // V3: initialization is awaited and failure is fatal — the
            // server refuses to bind sockets over an uninitialized vault.
            vault
                .initialize()
                .await
                .map_err(|e| VaultBuildError::InitFailed(e.to_string()))?;
            Ok(Box::new(vault))
        }
    }
}

/// V8: measure the vault instead of certifying it blind — a storage
/// round-trip (store → load → byte-compare) through the live
/// [`VaultInterface`] the server will actually serve from. Engine and audit
/// initialization are already proven by [`build_vault`] (V2/V3); this proves
/// the storage path end-to-end. The probe id is unique per boot because the
/// vault refuses duplicate model ids across restarts.
pub async fn probe_vault(vault: &dyn VaultInterface) -> crate::production_guard::RuntimeOutcome {
    use crate::production_guard::RuntimeOutcome;
    let probe_id = format!(
        "__mai_readiness_probe_{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let payload = b"mai vault readiness probe v1";
    if let Err(e) = vault.store_model_package(&probe_id, payload).await {
        return RuntimeOutcome::fail(format!("vault store probe failed: {e}"));
    }
    match vault.load_model_weights(&probe_id).await {
        Ok(bytes) if bytes == payload => RuntimeOutcome::pass(
            "vault storage round-trip measured (store + load + compare)".to_string(),
        ),
        Ok(_) => RuntimeOutcome::fail("vault probe read back different bytes".to_string()),
        Err(e) => RuntimeOutcome::fail(format!("vault load probe failed: {e}")),
    }
}

/// Derive a [`MaiVaultConfig`] from a single root directory.
///
/// All sub-paths rebase onto `root` so the builder can construct a
/// `ZfsVault` from just the profile's `vault.root`. Full TOML-driven
/// wiring of the rest of `VaultConfig` is tracked separately.
fn zfs_config_from_root(root: &Path) -> MaiVaultConfig {
    let mut cfg = MaiVaultConfig::default();
    cfg.storage.mount_point = root.join("models");
    cfg.storage.staging_dir = root.join("staging");
    cfg.pqc.key_store_path = root.join("keys");
    cfg.profiles.db_path = root.join("profiles.db");
    cfg.audit.db_path = root.join("audit.db");
    cfg
}

/// Stub vault used only when the profile is local-dev and explicitly
/// opts into `allow_stub=true`. Behavior matches the legacy
/// `server::StubVault`; the legacy one is retired in the
/// convergence step.
struct LocalDevStubVault;

#[async_trait]
impl VaultInterface for LocalDevStubVault {
    async fn load_model_weights(&self, model_id: &str) -> Result<Vec<u8>, VaultError> {
        Err(VaultError::ModelNotFound(model_id.to_string()))
    }

    async fn store_model_package(&self, _model_id: &str, _data: &[u8]) -> Result<(), VaultError> {
        Ok(())
    }

    async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
        Ok(())
    }

    async fn verify_signature(&self, _data: &[u8], _signature: &[u8]) -> Result<bool, VaultError> {
        Ok(true)
    }
}
