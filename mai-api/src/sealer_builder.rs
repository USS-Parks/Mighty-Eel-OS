//! Audit-sealer builder.
//!
//! Select the [`StoreSealer`] implementation for the compliance audit
//! WAL based on the parsed [`ShipProfile`]. This is the sealer analog
//! of [`crate::vault_builder`].
//!
//! Scope:
//! - Helper function [`build_sealer`].
//! - Production profile rejects [`NullSealer`] and demands a real
//!   AEAD key on disk.
//! - Local-dev profile permits [`NullSealer`] only when
//!   `audit.allow_null_sealer=true`; otherwise an ephemeral
//!   [`AeadSealer`] is constructed so dev workflows still exercise
//!   the encryption path.
//! - Conventional key location: `<audit.wal_dir>/sealer.key`.
//!
//! Out of scope:
//! - Wiring into [`crate::server::MaiServer`]. The convergence
//!   step swaps the current `NullSealer` defaults in the audit
//!   bootstrap for the builder's output.
//! - Vault-managed key acquisition. The key file is the bring-up
//!   contract; the convergence step folds it under the vault seal.
//! - Production-guard runtime check IDs (`PROD-AUDIT-*`). The
//!   audit-init guard wiring is owned separately; the sealer check ID
//!   is added at convergence.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mai_compliance::audit::{AEAD_SEALER_KEY_LEN, AeadSealer, NullSealer, StoreSealer};

use crate::ship_profile::{ProfileMode, ShipProfile};

/// Errors produced by [`build_sealer`] when a profile cannot yield a
/// usable sealer for the requested mode.
#[derive(Debug, thiserror::Error)]
pub enum SealerBuildError {
    /// Production profile set `audit.allow_null_sealer=true`. The
    /// parse-time validator already rejects this; the builder is the
    /// runtime second line of defense.
    #[error("production profile sets audit.allow_null_sealer=true; refused by the sealer builder")]
    NullSealerAllowedInProduction,
    /// Production profile selected an AEAD sealer but the key file
    /// does not exist on disk.
    #[error("sealer key file {path:?} does not exist; create it before boot in production")]
    KeyFileMissing {
        /// Conventional key path the builder expected.
        path: PathBuf,
    },
    /// Production profile selected an AEAD sealer but the key file
    /// could not be read.
    #[error("sealer key file {path:?} could not be read: {source}")]
    KeyFileIo {
        /// Path the builder tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Production profile selected an AEAD sealer but the key file
    /// length is wrong.
    #[error("sealer key file {path:?} has {actual} bytes; expected exactly {expected}")]
    KeyFileLengthInvalid {
        /// Path the builder read.
        path: PathBuf,
        /// Required byte count ([`AEAD_SEALER_KEY_LEN`]).
        expected: usize,
        /// Byte count the file actually had.
        actual: usize,
    },
}

/// Conventional sealer key location: `<audit.wal_dir>/sealer.key`.
/// Exposed so operator tooling and tests target the same path.
pub fn sealer_key_path(profile: &ShipProfile) -> PathBuf {
    profile.audit.wal_dir.join("sealer.key")
}

/// Construct the audit-store sealer selected by the profile.
///
/// Behavior matrix:
///
/// | Mode        | allow_null_sealer | key file exists | Outcome                           |
/// |-------------|-------------------|-----------------|-----------------------------------|
/// | production  | false             | yes (32 B)      | [`AeadSealer`] from key file      |
/// | production  | false             | no              | [`KeyFileMissing`]                |
/// | production  | false             | yes (wrong len) | [`KeyFileLengthInvalid`]          |
/// | production  | true              | n/a             | [`NullSealerAllowedInProduction`] |
/// | local-dev   | true              | n/a             | [`NullSealer`]                    |
/// | local-dev   | false             | n/a             | [`AeadSealer`] (ephemeral key)    |
///
/// [`KeyFileMissing`]: SealerBuildError::KeyFileMissing
/// [`KeyFileLengthInvalid`]: SealerBuildError::KeyFileLengthInvalid
/// [`NullSealerAllowedInProduction`]: SealerBuildError::NullSealerAllowedInProduction
pub fn build_sealer(profile: &ShipProfile) -> Result<Arc<dyn StoreSealer>, SealerBuildError> {
    let is_production = matches!(profile.profile.mode, ProfileMode::Production);

    if is_production && profile.audit.allow_null_sealer {
        return Err(SealerBuildError::NullSealerAllowedInProduction);
    }

    if is_production {
        let path = sealer_key_path(profile);
        let key = read_key_file(&path)?;
        Ok(Arc::new(AeadSealer::new(&key)))
    } else if profile.audit.allow_null_sealer {
        Ok(Arc::new(NullSealer))
    } else {
        Ok(Arc::new(AeadSealer::with_ephemeral_key()))
    }
}

fn read_key_file(path: &Path) -> Result<[u8; AEAD_SEALER_KEY_LEN], SealerBuildError> {
    if !path.exists() {
        return Err(SealerBuildError::KeyFileMissing {
            path: path.to_path_buf(),
        });
    }
    let bytes = fs::read(path).map_err(|source| SealerBuildError::KeyFileIo {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.len() != AEAD_SEALER_KEY_LEN {
        return Err(SealerBuildError::KeyFileLengthInvalid {
            path: path.to_path_buf(),
            expected: AEAD_SEALER_KEY_LEN,
            actual: bytes.len(),
        });
    }
    let mut key = [0u8; AEAD_SEALER_KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}
