//! Minimal subset of the ship profile schema needed by `mai-admin
//! backup`. Vendored from `mai-api/src/ship_profile.rs`.
//!
//! Why not depend on mai-api directly? When this crate was authored the
//! parallel endpoint-and-cli session was mid-edit on
//! `mai-api/src/errors.rs`, which left the mai-api crate un-buildable
//! in the shared workspace. Inlining the read-only subset of the
//! profile we actually consume keeps this crate independent. The
//! duplication should be removed once the endpoint-and-cli session lands
//! and `mai_api::ShipProfile` is reachable again — at that point
//! `BackupSourceProfile` becomes a thin facade over the canonical
//! type.
//!
//! Schema laxity: every section uses `#[serde(default)]` and the
//! struct only declares the fields we need, so a future profile that
//! grows new sections still loads here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What `mai-admin backup` reads from `profile.toml`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BackupSourceProfile {
    #[serde(default)]
    pub profile: ProfileMeta,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub vault: VaultConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    #[serde(default)]
    pub trust: TrustConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileMeta {
    #[serde(default = "default_profile_name")]
    pub name: String,
}

impl Default for ProfileMeta {
    fn default() -> Self {
        Self {
            name: default_profile_name(),
        }
    }
}

fn default_profile_name() -> String {
    "ship".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PathsConfig {
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default = "default_config_dir")]
    pub config_dir: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            state_dir: default_state_dir(),
            config_dir: default_config_dir(),
        }
    }
}

fn default_state_dir() -> PathBuf {
    PathBuf::from("/var/lib/mai")
}

fn default_config_dir() -> PathBuf {
    PathBuf::from("/etc/mai")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultConfig {
    #[serde(default)]
    pub backend: String,
    #[serde(default = "default_vault_root")]
    pub root: PathBuf,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            backend: String::new(),
            root: default_vault_root(),
        }
    }
}

fn default_vault_root() -> PathBuf {
    PathBuf::from("/var/lib/mai/vault")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuditConfig {
    #[serde(default = "default_audit_wal_dir")]
    pub wal_dir: PathBuf,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            wal_dir: default_audit_wal_dir(),
        }
    }
}

fn default_audit_wal_dir() -> PathBuf {
    PathBuf::from("/var/lib/mai/audit")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrustConfig {
    #[serde(default = "default_anchors_dir")]
    pub anchors_dir: PathBuf,
    #[serde(default = "default_bundle_cache_dir")]
    pub bundle_cache_dir: PathBuf,
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            anchors_dir: default_anchors_dir(),
            bundle_cache_dir: default_bundle_cache_dir(),
        }
    }
}

fn default_anchors_dir() -> PathBuf {
    PathBuf::from("/etc/mai/trust-anchors")
}

fn default_bundle_cache_dir() -> PathBuf {
    PathBuf::from("/var/lib/mai/trust")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_keys_path")]
    pub auth_keys_path: PathBuf,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            auth_keys_path: default_auth_keys_path(),
        }
    }
}

fn default_auth_keys_path() -> PathBuf {
    PathBuf::from("/etc/mai/auth_keys.toml")
}

#[derive(Debug, Error)]
pub enum ProfileLoadError {
    #[error("could not read profile {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("could not parse profile {0}: {1}")]
    Parse(PathBuf, Box<toml::de::Error>),
}

/// Load a profile from a TOML file path. Tolerant of extra fields the
/// vendored schema does not declare.
pub fn load_backup_source_profile(path: &Path) -> Result<BackupSourceProfile, ProfileLoadError> {
    let text =
        std::fs::read_to_string(path).map_err(|e| ProfileLoadError::Io(path.to_path_buf(), e))?;
    parse_backup_source_profile(&text)
        .map_err(|e| ProfileLoadError::Parse(path.to_path_buf(), Box::new(e)))
}

pub fn parse_backup_source_profile(text: &str) -> Result<BackupSourceProfile, toml::de::Error> {
    toml::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_realistic_ship_profile() {
        let text = r#"
[profile]
name = "ship"
mode = "production"
fail_closed = true

[paths]
state_dir = "/var/lib/mai"
config_dir = "/etc/mai"
log_dir = "/var/log/mai"
run_dir = "/run/mai"
backup_dir = "/var/backups/mai"

[vault]
backend = "zfs"
root = "/var/lib/mai/vault"

[audit]
api_writer = "wal"
compliance_writer = "wal"
wal_dir = "/var/lib/mai/audit"

[trust]
anchors_dir = "/etc/mai/trust-anchors"
bundle_cache_dir = "/var/lib/mai/trust"
verifier = "ml-dsa"

[auth]
auth_keys_path = "/etc/mai/auth_keys.toml"
"#;
        let p = parse_backup_source_profile(text).unwrap();
        assert_eq!(p.profile.name, "ship");
        assert_eq!(p.paths.state_dir, PathBuf::from("/var/lib/mai"));
        assert_eq!(p.audit.wal_dir, PathBuf::from("/var/lib/mai/audit"));
        assert_eq!(p.trust.anchors_dir, PathBuf::from("/etc/mai/trust-anchors"));
        assert_eq!(
            p.auth.auth_keys_path,
            PathBuf::from("/etc/mai/auth_keys.toml")
        );
        assert_eq!(p.vault.root, PathBuf::from("/var/lib/mai/vault"));
    }

    #[test]
    fn missing_sections_use_defaults() {
        let p = parse_backup_source_profile("").unwrap();
        assert_eq!(p.profile.name, "ship");
        assert_eq!(p.paths.state_dir, PathBuf::from("/var/lib/mai"));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let text = r#"
[profile]
name = "local-dev"

[future_section]
not_yet_modelled = true
"#;
        let p = parse_backup_source_profile(text).unwrap();
        assert_eq!(p.profile.name, "local-dev");
    }
}
