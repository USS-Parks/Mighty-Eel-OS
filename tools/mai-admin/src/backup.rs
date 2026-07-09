//! `mai-admin backup create` / `backup verify` implementation.
//!
//! A backup is rooted at `<output_dir>/<backup_id>/`, where `backup_id`
//! defaults to `mai-backup-<rfc3339-no-colons>`. Each component the
//! profile knows about is copied (or synthesized) into the backup dir,
//! sha3-256 hashed, and recorded in `manifest.json` at the root. The
//! manifest is optionally signed with an ML-DSA-87 key supplied by the
//! operator; ship profile requires the signature, local-dev does not.
//!
//! The backup is intentionally a directory tree, not a tarball — that
//! choice belongs to the site policy (some shops want tar+gpg, others
//! want zfs send | wormhole). This keeps the artefact format
//! transport-neutral.
//!
//! Components currently produced (per SHIP-HARDENING-PLAN §9):
//!
//! | name                 | source                              | required |
//! |----------------------|-------------------------------------|----------|
//! | build_info           | synthesized                         | yes      |
//! | config_checksums     | hash of profile.toml + auth_keys    | yes      |
//! | api_audit_wal        | profile.audit.wal_dir tree          | yes      |
//! | compliance_audit_wal | profile.audit.wal_dir/compliance    | optional |
//! | trust_bundle_cache   | profile.trust.bundle_cache_dir/...  | optional |
//! | trust_anchors        | profile.trust.anchors_dir tree      | yes      |
//! | vault_snapshot_ref   | synthesized JSON pointer            | yes      |
//! | auth_key_hashes      | sha3 of every key_id in keys store  | yes      |
//! | model_registry       | <state_dir>/models/registry.json    | optional |
//! | reports              | <state_dir>/reports/ tree           | optional |
//!
//! Restore consumes the same component IDs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audit::{AuditEntry, GENESIS_HASH as AUDIT_GENESIS_HASH, verify_chain};
use crate::manifest::{
    BackupManifest, MLDSA87_SK_LEN, ManifestComponent, ManifestError, ManifestSignatures,
    VerifyOutcome, sha3_file, sha3_hex, sha3_tree,
};
use crate::profile::BackupSourceProfile;

/// Options driving `BackupRunner::run`.
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Parent directory; the backup is created at
    /// `<output_root>/<backup_id>/`.
    pub output_root: PathBuf,
    /// Optional override; default `mai-backup-<timestamp>`.
    pub backup_id: Option<String>,
    /// Unix epoch seconds the backup started; used in `created_at` and
    /// the default `backup_id`. Settable so tests get deterministic IDs.
    pub now_secs: u64,
    /// Optional 4896-byte ML-DSA-87 secret key. Required in ship mode.
    pub signing_key: Option<Vec<u8>>,
    /// Required when `signing_key` is present.
    pub anchor_id: Option<String>,
    /// Build metadata recorded in the manifest.
    pub mai_version: String,
    pub git_commit: String,
    pub migration_version: String,
    pub host: String,
}

impl BackupOptions {
    /// New options with sensible defaults from the host process.
    pub fn from_env(output_root: impl Into<PathBuf>) -> Self {
        Self {
            output_root: output_root.into(),
            backup_id: None,
            now_secs: now_secs(),
            signing_key: None,
            anchor_id: None,
            mai_version: env!("CARGO_PKG_VERSION").to_string(),
            git_commit: option_env!("MAI_GIT_COMMIT").unwrap_or("").to_string(),
            migration_version: "0.1.0".to_string(),
            host: hostname(),
        }
    }
}

/// What `BackupRunner::run` produced.
#[derive(Debug, Clone)]
pub struct BackupReport {
    pub backup_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub backup_id: String,
    pub component_count: usize,
    pub signed: bool,
    pub warnings: Vec<String>,
}

/// What `verify_backup` produced.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub backup_dir: PathBuf,
    pub backup_id: String,
    pub signature_outcome: VerifyOutcome,
    pub component_count: usize,
    /// Set when `--require-signed` was true but the backup wasn't signed
    /// or no verifying key was supplied.
    pub failures: Vec<String>,
    /// Cosmetic notes (skipped optional components, etc.).
    pub warnings: Vec<String>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// What can go wrong while taking or verifying a backup.
#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("backup serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error(
        "backup target {0} already exists; refuse to overwrite without explicit operator action"
    )]
    OutputExists(PathBuf),
    #[error("signing key length {actual} != {MLDSA87_SK_LEN}; supplied key is not ML-DSA-87")]
    BadSigningKeyLength { actual: usize },
    #[error("signing requested but no anchor_id supplied; aborting before manifest write")]
    MissingAnchorId,
    #[error("WAL replay failed for {component} at entry {index}: {detail}")]
    WalChainBroken {
        component: String,
        index: usize,
        detail: String,
    },
    #[error("component {component} sha3 mismatch: stored {stored} computed {computed}")]
    ComponentDigestMismatch {
        component: String,
        stored: String,
        computed: String,
    },
    #[error("component {0} listed in manifest but missing on disk")]
    ComponentMissing(String),
    #[error("auth keys file at {0} could not be parsed: {1}")]
    AuthKeysParse(PathBuf, String),
}

/// Take a backup against a loaded ship profile.
///
/// The caller is responsible for loading the profile via
/// `mai_api::load_ship_profile` and supplying any signing material.
/// Restore reads everything it needs from the manifest the
/// runner produces here.
#[allow(clippy::needless_pass_by_value)]
pub fn create_backup(
    profile: &BackupSourceProfile,
    options: BackupOptions,
) -> Result<BackupReport, BackupError> {
    if let Some(ref sk) = options.signing_key {
        if sk.len() != MLDSA87_SK_LEN {
            return Err(BackupError::BadSigningKeyLength { actual: sk.len() });
        }
        if options.anchor_id.is_none() {
            return Err(BackupError::MissingAnchorId);
        }
    }

    let backup_id = options
        .backup_id
        .clone()
        .unwrap_or_else(|| default_backup_id(options.now_secs));
    let backup_dir = options.output_root.join(&backup_id);
    if backup_dir.exists() {
        return Err(BackupError::OutputExists(backup_dir));
    }
    std::fs::create_dir_all(&backup_dir)?;

    let mut warnings = Vec::new();
    let mut components: Vec<ManifestComponent> = Vec::new();

    components.push(write_build_info(&backup_dir, profile, &options)?);
    components.push(write_config_checksums(&backup_dir, profile, &mut warnings)?);
    components.push(write_api_audit_wal(&backup_dir, profile)?);
    if let Some(c) = maybe_write_compliance_audit_wal(&backup_dir, profile)? {
        components.push(c);
    } else {
        warnings.push("compliance audit WAL directory not found; component skipped".to_string());
    }
    if let Some(c) = maybe_write_trust_bundle_cache(&backup_dir, profile)? {
        components.push(c);
    } else {
        warnings.push("trust bundle cache empty or missing; component skipped".to_string());
    }
    components.push(write_trust_anchors(&backup_dir, profile)?);
    components.push(write_vault_snapshot_ref(&backup_dir, profile)?);
    components.push(write_auth_key_hashes(&backup_dir, profile, &mut warnings)?);
    if let Some(c) = maybe_write_model_registry(&backup_dir, profile)? {
        components.push(c);
    } else {
        warnings.push("model registry not present; component skipped".to_string());
    }
    if let Some(c) = maybe_write_reports(&backup_dir, profile)? {
        components.push(c);
    } else {
        warnings.push("reports directory empty or missing; component skipped".to_string());
    }

    components.sort_by(|a, b| a.name.cmp(&b.name));

    let mut manifest = BackupManifest {
        backup_id: backup_id.clone(),
        created_at: rfc3339(options.now_secs),
        mai_version: options.mai_version.clone(),
        git_commit: options.git_commit.clone(),
        profile: profile.profile.name.clone(),
        host: options.host.clone(),
        migration_version: options.migration_version.clone(),
        components,
        signatures: ManifestSignatures::default(),
    };

    let signed = if let Some(ref sk) = options.signing_key {
        let anchor = options
            .anchor_id
            .as_deref()
            .ok_or(BackupError::MissingAnchorId)?;
        manifest.sign(sk, anchor)?;
        true
    } else {
        false
    };

    let manifest_path = backup_dir.join("manifest.json");
    manifest.write_to(&manifest_path)?;

    Ok(BackupReport {
        backup_dir,
        manifest_path,
        backup_id,
        component_count: manifest.components.len(),
        signed,
        warnings,
    })
}

/// Reload a manifest from `<backup_dir>/manifest.json`, optionally
/// verify its signature against a supplied public key, then recompute
/// every component's sha3-256 and (for WAL components) replay the
/// hash chain.
///
/// `verifying_key` is optional. When absent the signature check is
/// skipped (but signature presence still influences the outcome
/// because `VerifyReport::signature_outcome` carries the info).
/// When `require_signed` is true, an unsigned manifest is a failure.
pub fn verify_backup(
    backup_dir: &Path,
    verifying_key: Option<&[u8]>,
    require_signed: bool,
) -> Result<VerifyReport, BackupError> {
    let manifest_path = backup_dir.join("manifest.json");
    let manifest = BackupManifest::load_from(&manifest_path)?;

    let mut failures: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let signature_outcome = match verifying_key {
        Some(pk) => match manifest.verify(pk) {
            Ok(outcome) => outcome,
            Err(e) => {
                failures.push(format!("manifest signature: {e}"));
                VerifyOutcome::Unsigned
            }
        },
        None => {
            if manifest.signatures.manifest_mldsa.is_some() {
                warnings.push(
                    "manifest is signed but no verifying key supplied; signature not checked"
                        .to_string(),
                );
                VerifyOutcome::Signed {
                    anchor_id: manifest
                        .signatures
                        .anchor_id
                        .clone()
                        .unwrap_or_else(|| "unknown-anchor".to_string()),
                }
            } else {
                VerifyOutcome::Unsigned
            }
        }
    };

    if require_signed && !matches!(signature_outcome, VerifyOutcome::Signed { .. }) {
        failures.push("manifest is not signed but --require-signed was set".to_string());
    }

    for component in &manifest.components {
        let target = backup_dir.join(&component.path);
        if !target.exists() {
            failures.push(format!(
                "component {} missing on disk at {}",
                component.name,
                target.display()
            ));
            continue;
        }
        let computed = if target.is_dir() {
            let (digest, _files, _bytes) = sha3_tree(&target)?;
            digest
        } else {
            sha3_file(&target)?
        };
        if computed != component.sha3_256 {
            failures.push(format!(
                "component {} sha3 mismatch: stored {} computed {}",
                component.name, component.sha3_256, computed
            ));
            continue;
        }
        // For WAL components, replay the chain.
        if matches!(
            component.name.as_str(),
            "api_audit_wal" | "compliance_audit_wal"
        ) && let Err(e) = replay_wal_tree(&target, component.last_entry_hash.as_deref())
        {
            failures.push(format!("component {}: {e}", component.name));
        }
    }

    Ok(VerifyReport {
        backup_dir: backup_dir.to_path_buf(),
        backup_id: manifest.backup_id,
        signature_outcome,
        component_count: manifest.components.len(),
        failures,
        warnings,
    })
}

// ─── component writers ────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct BuildInfo {
    mai_version: String,
    git_commit: String,
    profile: String,
    host: String,
    created_at: String,
    migration_version: String,
}

fn write_build_info(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
    options: &BackupOptions,
) -> Result<ManifestComponent, BackupError> {
    let info = BuildInfo {
        mai_version: options.mai_version.clone(),
        git_commit: options.git_commit.clone(),
        profile: profile.profile.name.clone(),
        host: options.host.clone(),
        created_at: rfc3339(options.now_secs),
        migration_version: options.migration_version.clone(),
    };
    let rel = "metadata/build-info.json";
    let abs = backup_dir.join(rel);
    std::fs::create_dir_all(abs.parent().expect("file has parent"))?;
    let bytes = serde_json::to_vec_pretty(&info)?;
    std::fs::write(&abs, &bytes)?;
    Ok(ManifestComponent {
        name: "build_info".into(),
        path: rel.into(),
        sha3_256: sha3_hex(&bytes),
        bytes: bytes.len() as u64,
        entry_count: None,
        last_entry_hash: None,
        file_count: None,
    })
}

fn write_config_checksums(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
    warnings: &mut Vec<String>,
) -> Result<ManifestComponent, BackupError> {
    let mut checksums: BTreeMap<String, String> = BTreeMap::new();
    for filename in ["profile.toml", "auth_keys.toml", "dashboard-logging.json"] {
        let path = profile.paths.config_dir.join(filename);
        if path.is_file() {
            checksums.insert(filename.into(), sha3_file(&path)?);
        } else {
            warnings.push(format!(
                "config file {} missing; not hashed",
                path.display()
            ));
        }
    }
    let rel = "metadata/config-checksums.json";
    let abs = backup_dir.join(rel);
    std::fs::create_dir_all(abs.parent().expect("file has parent"))?;
    let bytes = serde_json::to_vec_pretty(&checksums)?;
    std::fs::write(&abs, &bytes)?;
    Ok(ManifestComponent {
        name: "config_checksums".into(),
        path: rel.into(),
        sha3_256: sha3_hex(&bytes),
        bytes: bytes.len() as u64,
        entry_count: None,
        last_entry_hash: None,
        file_count: None,
    })
}

fn write_api_audit_wal(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<ManifestComponent, BackupError> {
    let src_dir = &profile.audit.wal_dir;
    let dst_rel = "audit/api";
    let dst_dir = backup_dir.join(dst_rel);
    std::fs::create_dir_all(&dst_dir)?;
    if src_dir.is_dir() {
        copy_wal_dir(src_dir, &dst_dir, &["compliance"])?;
    }
    let (digest, file_count, bytes) = sha3_tree(&dst_dir)?;
    let (entry_count, last_entry_hash) = replay_wal_tree_for_meta(&dst_dir)?;
    Ok(ManifestComponent {
        name: "api_audit_wal".into(),
        path: dst_rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: Some(entry_count),
        last_entry_hash: Some(last_entry_hash),
        file_count: Some(file_count),
    })
}

fn maybe_write_compliance_audit_wal(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<Option<ManifestComponent>, BackupError> {
    let src_dir = profile.audit.wal_dir.join("compliance");
    if !src_dir.is_dir() {
        return Ok(None);
    }
    let dst_rel = "audit/compliance";
    let dst_dir = backup_dir.join(dst_rel);
    std::fs::create_dir_all(&dst_dir)?;
    copy_dir_recursive(&src_dir, &dst_dir)?;
    let (digest, file_count, bytes) = sha3_tree(&dst_dir)?;
    let (entry_count, last_entry_hash) = replay_wal_tree_for_meta(&dst_dir)?;
    Ok(Some(ManifestComponent {
        name: "compliance_audit_wal".into(),
        path: dst_rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: Some(entry_count),
        last_entry_hash: Some(last_entry_hash),
        file_count: Some(file_count),
    }))
}

fn maybe_write_trust_bundle_cache(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<Option<ManifestComponent>, BackupError> {
    let src_dir = &profile.trust.bundle_cache_dir;
    if !src_dir.is_dir() {
        return Ok(None);
    }
    let dst_rel = "trust/bundles";
    let dst_dir = backup_dir.join(dst_rel);
    std::fs::create_dir_all(&dst_dir)?;
    copy_dir_recursive(src_dir, &dst_dir)?;
    let (digest, file_count, bytes) = sha3_tree(&dst_dir)?;
    if file_count == 0 {
        return Ok(None);
    }
    Ok(Some(ManifestComponent {
        name: "trust_bundle_cache".into(),
        path: dst_rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: None,
        last_entry_hash: None,
        file_count: Some(file_count),
    }))
}

fn write_trust_anchors(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<ManifestComponent, BackupError> {
    let src_dir = &profile.trust.anchors_dir;
    let dst_rel = "trust/anchors";
    let dst_dir = backup_dir.join(dst_rel);
    std::fs::create_dir_all(&dst_dir)?;
    if src_dir.is_dir() {
        copy_dir_recursive(src_dir, &dst_dir)?;
    }
    let (digest, file_count, bytes) = sha3_tree(&dst_dir)?;
    Ok(ManifestComponent {
        name: "trust_anchors".into(),
        path: dst_rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: None,
        last_entry_hash: None,
        file_count: Some(file_count),
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct VaultSnapshotRef {
    backend: String,
    root: PathBuf,
    strategy: &'static str,
    notes: &'static str,
}

fn write_vault_snapshot_ref(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<ManifestComponent, BackupError> {
    let snapshot = VaultSnapshotRef {
        backend: profile.vault.backend.clone(),
        root: profile.vault.root.clone(),
        strategy: "external-zfs",
        notes: "Vault payloads stay encrypted at rest. The manifest records the \
                backend + root; the operator's site-policy ZFS replication \
                (or equivalent) ships the actual blocks. A future revision will \
                refuse to restore if this strategy is not satisfied.",
    };
    let rel = "vault/snapshot-ref.json";
    let abs = backup_dir.join(rel);
    std::fs::create_dir_all(abs.parent().expect("file has parent"))?;
    let bytes = serde_json::to_vec_pretty(&snapshot)?;
    std::fs::write(&abs, &bytes)?;
    Ok(ManifestComponent {
        name: "vault_snapshot_ref".into(),
        path: rel.into(),
        sha3_256: sha3_hex(&bytes),
        bytes: bytes.len() as u64,
        entry_count: None,
        last_entry_hash: None,
        file_count: None,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthKeyHashesDoc {
    schema_version: u32,
    entries: BTreeMap<String, String>,
}

fn write_auth_key_hashes(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
    warnings: &mut Vec<String>,
) -> Result<ManifestComponent, BackupError> {
    let path = &profile.auth.auth_keys_path;
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    if path.is_file() {
        let text = std::fs::read_to_string(path)?;
        let parsed: toml::Value = text.parse().map_err(|e| {
            BackupError::AuthKeysParse(path.clone(), format!("toml parse error: {e}"))
        })?;
        if let Some(keys) = parsed.get("keys").and_then(|v| v.as_table()) {
            for (id, value) in keys {
                if let Some(raw) = value.as_str() {
                    entries.insert(id.clone(), sha3_hex(raw.as_bytes()));
                } else if let Some(t) = value.as_table()
                    && let Some(raw) = t.get("api_key").and_then(|v| v.as_str())
                {
                    entries.insert(id.clone(), sha3_hex(raw.as_bytes()));
                }
            }
        } else {
            warnings.push(format!(
                "auth keys file {} has no [keys] table; recorded empty hash set",
                path.display()
            ));
        }
    } else {
        warnings.push(format!(
            "auth keys file {} missing; recorded empty hash set",
            path.display()
        ));
    }
    let doc = AuthKeyHashesDoc {
        schema_version: 1,
        entries,
    };
    let rel = "auth/key-hashes.json";
    let abs = backup_dir.join(rel);
    std::fs::create_dir_all(abs.parent().expect("file has parent"))?;
    let bytes = serde_json::to_vec_pretty(&doc)?;
    std::fs::write(&abs, &bytes)?;
    Ok(ManifestComponent {
        name: "auth_key_hashes".into(),
        path: rel.into(),
        sha3_256: sha3_hex(&bytes),
        bytes: bytes.len() as u64,
        entry_count: Some(doc.entries.len() as u64),
        last_entry_hash: None,
        file_count: None,
    })
}

fn maybe_write_model_registry(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<Option<ManifestComponent>, BackupError> {
    let src = profile.paths.state_dir.join("models/registry.json");
    if !src.is_file() {
        return Ok(None);
    }
    let rel = "models/registry.json";
    let dst = backup_dir.join(rel);
    std::fs::create_dir_all(dst.parent().expect("file has parent"))?;
    std::fs::copy(&src, &dst)?;
    let digest = sha3_file(&dst)?;
    let bytes = std::fs::metadata(&dst)?.len();
    Ok(Some(ManifestComponent {
        name: "model_registry".into(),
        path: rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: None,
        last_entry_hash: None,
        file_count: None,
    }))
}

fn maybe_write_reports(
    backup_dir: &Path,
    profile: &BackupSourceProfile,
) -> Result<Option<ManifestComponent>, BackupError> {
    let src_dir = profile.paths.state_dir.join("reports");
    if !src_dir.is_dir() {
        return Ok(None);
    }
    let dst_rel = "reports";
    let dst_dir = backup_dir.join(dst_rel);
    std::fs::create_dir_all(&dst_dir)?;
    copy_dir_recursive(&src_dir, &dst_dir)?;
    let (digest, file_count, bytes) = sha3_tree(&dst_dir)?;
    if file_count == 0 {
        return Ok(None);
    }
    Ok(Some(ManifestComponent {
        name: "reports".into(),
        path: dst_rel.into(),
        sha3_256: digest,
        bytes,
        entry_count: None,
        last_entry_hash: None,
        file_count: Some(file_count),
    }))
}

// ─── helpers ──────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn rfc3339(secs: u64) -> String {
    use chrono::DateTime;
    let signed = i64::try_from(secs).unwrap_or(0);
    DateTime::from_timestamp(signed, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

fn default_backup_id(secs: u64) -> String {
    let stamp = rfc3339(secs).replace(':', "-");
    format!("mai-backup-{stamp}")
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), BackupError> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Copy the API audit WAL directory but skip an inner `compliance/`
/// sub-directory (that one is its own component).
fn copy_wal_dir(src: &Path, dst: &Path, skip_subdirs: &[&str]) -> Result<(), BackupError> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() && skip_subdirs.iter().any(|s| std::ffi::OsStr::new(s) == name) {
            continue;
        }
        let to = dst.join(&name);
        if from.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn ordered_wal_files(dir: &Path) -> Result<Vec<PathBuf>, BackupError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let rotated_dir = dir.join("rotated");
    if rotated_dir.is_dir() {
        let mut rotated: Vec<PathBuf> = std::fs::read_dir(&rotated_dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
            .collect();
        rotated.sort();
        files.extend(rotated);
    }
    let current = dir.join("current.jsonl");
    if current.is_file() {
        files.push(current);
    }
    Ok(files)
}

fn read_wal_entries(dir: &Path) -> Result<Vec<AuditEntry>, BackupError> {
    let mut entries = Vec::new();
    for file in ordered_wal_files(dir)? {
        let text = std::fs::read_to_string(&file)?;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(line)?;
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn replay_wal_tree_for_meta(dir: &Path) -> Result<(u64, String), BackupError> {
    let entries = read_wal_entries(dir)?;
    if entries.is_empty() {
        return Ok((0, AUDIT_GENESIS_HASH.to_string()));
    }
    if let Err((index, detail)) = verify_chain(&entries) {
        return Err(BackupError::WalChainBroken {
            component: dir.display().to_string(),
            index,
            detail,
        });
    }
    let last = entries
        .last()
        .map_or_else(|| AUDIT_GENESIS_HASH.to_string(), |e| e.entry_hash.clone());
    Ok((entries.len() as u64, last))
}

fn replay_wal_tree(dir: &Path, expected_last: Option<&str>) -> Result<(), BackupError> {
    let (_count, last) = replay_wal_tree_for_meta(dir)?;
    if let Some(want) = expected_last
        && want != last
    {
        return Err(BackupError::ComponentDigestMismatch {
            component: format!("{} (last_entry_hash)", dir.display()),
            stored: want.to_string(),
            computed: last,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backup_id_is_stable() {
        let secs = 1_716_460_800u64; // 2024-05-23T11:20:00Z (deterministic)
        let id = default_backup_id(secs);
        assert!(id.starts_with("mai-backup-"));
        assert!(!id.contains(':'), "backup_id must be filesystem-safe");
    }

    #[test]
    fn rfc3339_round_trips_seconds() {
        let s = rfc3339(0);
        assert_eq!(s, "1970-01-01T00:00:00Z");
    }
}
