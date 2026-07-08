//! `mai-admin restore plan` / `restore apply` implementation. SHIP-10.
//!
//! Restore is the inverse of [`crate::backup::create_backup`]: it
//! reloads the manifest at `<backup_dir>/manifest.json`, verifies the
//! signature + per-component sha3 digests + audit chain (refusing to
//! touch the target on any failure), then writes every component file
//! or tree into `<target_dir>/<component.path>` and produces a signed-
//! shape `restore-report.json` next to the components.
//!
//! Two-phase by design:
//! * [`plan_restore`] is read-only. It loads the manifest, verifies the
//!   *backup-side* contents in full, and scans the target for any
//!   conflicting files. Operators use this for dry runs.
//! * [`apply_restore`] executes the plan. It refuses to overwrite a
//!   populated target unless `force` is true, recomputes each
//!   component digest *after* the write (catching in-flight corruption
//!   that bypasses the source-side check), and replays the audit chain
//!   in the *restored* tree.
//!
//! After a successful apply the operator can boot a `mai-api` against
//! the restored state with a profile that points at
//! `<target>/audit/api`, `<target>/trust/anchors`, etc. — exactly the
//! same relative layout the backup recorded. SHIP-HARDENING-PLAN §16
//! demands `mai-ship-validate --state-dir <target> --profile …` after
//! restore; the restore-report records the inputs that gate validates.
//!
//! Component layout in the target mirrors the backup byte-for-byte
//! (audit/api/, audit/compliance/, trust/anchors/, trust/bundles/,
//! auth/key-hashes.json, vault/snapshot-ref.json, metadata/build-info.json,
//! metadata/config-checksums.json, models/registry.json, reports/).
//! The `manifest.json` itself is copied to the target as
//! `source-manifest.json` so subsequent verifies have a witness of
//! what was promised.
//!
//! What restore deliberately does NOT do:
//! * It does not reseal vault payloads. Vault data stays at rest under
//!   its own backend (ZFS replication or whatever the operator runs);
//!   the backup records only a `vault_snapshot_ref` pointer. SHIP-09's
//!   discipline carries forward unchanged.
//! * It does not write a `profile.toml`. The operator supplies a fresh
//!   profile (or symlinks an existing one) whose paths point at the
//!   restored layout. This keeps restore environment-agnostic.
//! * It does not chown / chmod. Filesystem permissions are the
//!   operator's responsibility — packaging (SHIP-08) sets them up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::backup::BackupError;
use crate::manifest::{BackupManifest, ManifestError, VerifyOutcome, sha3_file, sha3_tree};

/// One write the plan intends to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreAction {
    /// Component name, matching `ManifestComponent::name`.
    pub name: String,
    /// Path inside the backup directory, relative to the backup root.
    /// Always the same value as `target_relative`.
    pub source_in_backup: PathBuf,
    /// Where this component lands inside the target, relative to the
    /// target root. Same as the manifest path; restore mirrors layout.
    pub target_relative: PathBuf,
    /// Whether the component is a single file or a directory tree.
    pub kind: ActionKind,
    /// Expected sha3 (hex) — apply-time post-write check.
    pub expected_sha3: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    File,
    Tree,
}

/// Validate a manifest-supplied component path (finding AF-11). It must be a
/// relative path composed only of `Normal` components — no absolute/root, no
/// drive prefix, no `.`/`..` — so joining it onto the backup or target root
/// cannot escape that root. Returns the validated relative path.
fn validate_component_path(raw: &str) -> Result<PathBuf, RestoreError> {
    use std::path::Component;
    let p = Path::new(raw);
    if raw.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(RestoreError::UnsafeComponentPath(raw.to_string()));
    }
    Ok(p.to_path_buf())
}

/// A path inside the target that already exists and would be
/// overwritten without `force`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreObstacle {
    /// Absolute path inside the target.
    pub path: PathBuf,
    /// Why the obstacle was flagged (existing file, populated dir, etc.).
    pub reason: String,
}

/// What `plan_restore` produced.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    pub backup_dir: PathBuf,
    pub target_dir: PathBuf,
    pub backup_id: String,
    pub signature_outcome: VerifyOutcome,
    pub actions: Vec<RestoreAction>,
    pub obstacles: Vec<RestoreObstacle>,
    /// Non-blocking notes: unsigned manifest with no verifier supplied,
    /// optional components skipped on backup, etc.
    pub warnings: Vec<String>,
    /// Cached manifest. apply_restore reads from this so plan + apply
    /// share one consistent view.
    pub manifest: BackupManifest,
}

impl RestorePlan {
    /// True iff `apply_restore` would refuse without `force`.
    pub fn has_blocking_obstacles(&self) -> bool {
        !self.obstacles.is_empty()
    }
}

/// What `apply_restore` produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub backup_id: String,
    pub backup_dir: String,
    pub target_dir: String,
    pub signature_outcome: RestoreSignatureRecord,
    pub restored_components: Vec<RestoredComponent>,
    pub warnings: Vec<String>,
    /// True iff every WAL component (api + compliance) replayed
    /// successfully against the restored tree.
    pub audit_chain_verified: bool,
    /// True iff `force` was used to overwrite an existing target.
    pub forced_overwrite: bool,
    /// RFC 3339 timestamp the apply finished.
    pub completed_at: String,
    /// The mai-admin version that produced the restore. Independent of
    /// the producing-backup version recorded in `manifest.mai_version`.
    pub restored_by_version: String,
}

/// JSON-serialisable mirror of `VerifyOutcome` for the restore report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreSignatureRecord {
    Signed { anchor_id: String },
    Unsigned,
}

impl From<&VerifyOutcome> for RestoreSignatureRecord {
    fn from(o: &VerifyOutcome) -> Self {
        match o {
            VerifyOutcome::Signed { anchor_id } => Self::Signed {
                anchor_id: anchor_id.clone(),
            },
            VerifyOutcome::Unsigned => Self::Unsigned,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoredComponent {
    pub name: String,
    pub target_relative: String,
    /// `"file"` or `"tree"`. Stored as `String` (not `&'static str`)
    /// because `RestoreReport` is `Deserialize` for round-trip tests.
    pub kind: String,
    pub sha3_256: String,
    pub bytes: u64,
    /// For WAL components: the last entry hash observed in the restored
    /// tree. Must match the manifest's `last_entry_hash`; restore aborts
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_entry_hash: Option<String>,
}

/// What can go wrong while planning or applying a restore.
#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("restore io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("restore serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("backup error: {0}")]
    Backup(#[from] BackupError),
    #[error("backup directory {0} does not contain a manifest.json")]
    ManifestMissing(PathBuf),
    #[error(
        "target {path} already contains files; pass --force to overwrite \
         (refused: {reason})"
    )]
    TargetNotEmpty { path: PathBuf, reason: String },
    #[error("signed manifest required but backup is unsigned")]
    UnsignedManifest,
    #[error("manifest signature failed: {0}")]
    SignatureFailed(String),
    #[error(
        "backup component {name} sha3 mismatch (source): stored {stored} computed {computed}; \
         the backup itself is corrupt"
    )]
    SourceDigestMismatch {
        name: String,
        stored: String,
        computed: String,
    },
    #[error(
        "restored component {name} sha3 mismatch (target): stored {stored} computed {computed}; \
         write was corrupted in flight"
    )]
    TargetDigestMismatch {
        name: String,
        stored: String,
        computed: String,
    },
    #[error("backup component {0} is missing on disk")]
    SourceMissing(String),
    #[error("backup component path {0:?} is unsafe (escapes the backup/target root)")]
    UnsafeComponentPath(String),
    #[error("audit chain replay failed for {component} at entry {index}: {detail}")]
    AuditChainBroken {
        component: String,
        index: usize,
        detail: String,
    },
    #[error(
        "audit chain last entry mismatch for {component}: manifest says {stored}, \
         restored tree replays to {computed}"
    )]
    AuditChainLastMismatch {
        component: String,
        stored: String,
        computed: String,
    },
}

/// Build a [`RestorePlan`] from `backup_dir` against `target_dir`.
///
/// The function is read-only: it never writes anything to `target_dir`.
/// All verifications run *before* the obstacle scan, so a corrupt backup
/// fails fast and the operator never sees a misleading "target is fine"
/// success on bad input.
///
/// * `verifying_key` — optional 2592-byte ML-DSA-87 public key. When
///   present the manifest signature is verified; when absent (and
///   `require_signed` is false) the manifest is accepted unsigned with
///   a warning.
/// * `require_signed` — when true, an unsigned manifest (or a missing
///   `verifying_key`) is a hard failure. Ship profile should always
///   pass `true`.
pub fn plan_restore(
    backup_dir: &Path,
    target_dir: &Path,
    verifying_key: Option<&[u8]>,
    require_signed: bool,
) -> Result<RestorePlan, RestoreError> {
    let manifest_path = backup_dir.join("manifest.json");
    if !manifest_path.is_file() {
        return Err(RestoreError::ManifestMissing(backup_dir.to_path_buf()));
    }
    let manifest = BackupManifest::load_from(&manifest_path)?;
    let mut warnings: Vec<String> = Vec::new();

    // Signature first — a tampered manifest taints the whole plan.
    let signature_outcome = match verifying_key {
        Some(pk) => manifest
            .verify(pk)
            .map_err(|e| RestoreError::SignatureFailed(e.to_string()))?,
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
    if require_signed {
        match &signature_outcome {
            VerifyOutcome::Unsigned => return Err(RestoreError::UnsignedManifest),
            VerifyOutcome::Signed { .. } if verifying_key.is_none() => {
                return Err(RestoreError::SignatureFailed(
                    "manifest claims signature but no verifying key supplied".to_string(),
                ));
            }
            VerifyOutcome::Signed { .. } => {}
        }
    }

    // Source-side digest + WAL replay. We do this in plan() rather than
    // apply() so a corrupt backup cannot ever touch the target.
    let mut actions: Vec<RestoreAction> = Vec::with_capacity(manifest.components.len());
    for component in &manifest.components {
        // AF-11: refuse any component path that could escape the backup/target
        // root before it is joined onto either.
        let safe_rel = validate_component_path(&component.path)?;
        let source_abs = backup_dir.join(&safe_rel);
        if !source_abs.exists() {
            return Err(RestoreError::SourceMissing(component.name.clone()));
        }
        let (kind, computed) = if source_abs.is_dir() {
            let (digest, _files, _bytes) = sha3_tree(&source_abs)?;
            (ActionKind::Tree, digest)
        } else {
            (ActionKind::File, sha3_file(&source_abs)?)
        };
        if computed != component.sha3_256 {
            return Err(RestoreError::SourceDigestMismatch {
                name: component.name.clone(),
                stored: component.sha3_256.clone(),
                computed,
            });
        }
        if matches!(
            component.name.as_str(),
            "api_audit_wal" | "compliance_audit_wal"
        ) {
            replay_chain(&source_abs, &component.name, &component.last_entry_hash)?;
        }
        actions.push(RestoreAction {
            name: component.name.clone(),
            source_in_backup: safe_rel.clone(),
            target_relative: safe_rel,
            kind,
            expected_sha3: component.sha3_256.clone(),
        });
    }

    // Obstacle scan. We surface the *first* conflicting path per
    // component; a single conflicting tree is enough signal for the
    // operator.
    let mut obstacles: Vec<RestoreObstacle> = Vec::new();
    if target_dir.exists() {
        // Manifest itself never overwrites; we write `source-manifest.json`.
        // restore-report.json is a fresh emit, also new.
        for action in &actions {
            let target = target_dir.join(&action.target_relative);
            match action.kind {
                ActionKind::File => {
                    if target.exists() {
                        obstacles.push(RestoreObstacle {
                            path: target,
                            reason: format!("file already present for component {}", action.name),
                        });
                    }
                }
                ActionKind::Tree => {
                    if target.is_dir() && dir_has_entries(&target)? {
                        obstacles.push(RestoreObstacle {
                            path: target,
                            reason: format!(
                                "directory already populated for component {}",
                                action.name
                            ),
                        });
                    }
                }
            }
        }
    }

    Ok(RestorePlan {
        backup_dir: backup_dir.to_path_buf(),
        target_dir: target_dir.to_path_buf(),
        backup_id: manifest.backup_id.clone(),
        signature_outcome,
        actions,
        obstacles,
        warnings,
        manifest,
    })
}

/// Execute a [`RestorePlan`].
///
/// * `force` — when true, existing files inside the target are
///   overwritten and existing directory trees are merged-and-replaced
///   on a per-file basis. When false, any obstacle in the plan causes
///   the function to return [`RestoreError::TargetNotEmpty`] *before*
///   touching the filesystem.
///
/// On success the target ends up containing:
/// * One file or directory tree per component, mirroring the backup's
///   relative layout.
/// * `<target>/source-manifest.json` — a copy of the backup manifest
///   so a subsequent verify has a witness of what was promised.
/// * `<target>/restore-report.json` — the [`RestoreReport`] this call
///   returns, serialized for audit.
pub fn apply_restore(plan: &RestorePlan, force: bool) -> Result<RestoreReport, RestoreError> {
    if plan.has_blocking_obstacles() && !force {
        let first = plan
            .obstacles
            .first()
            .map(|o| o.reason.clone())
            .unwrap_or_else(|| "unknown obstacle".to_string());
        return Err(RestoreError::TargetNotEmpty {
            path: plan.target_dir.clone(),
            reason: first,
        });
    }

    std::fs::create_dir_all(&plan.target_dir)?;

    let mut restored: Vec<RestoredComponent> = Vec::with_capacity(plan.actions.len());
    for action in &plan.actions {
        let source_abs = plan.backup_dir.join(&action.source_in_backup);
        let target_abs = plan.target_dir.join(&action.target_relative);
        match action.kind {
            ActionKind::File => {
                if let Some(parent) = target_abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if target_abs.exists() {
                    // Forced overwrite path: remove first so we don't
                    // leak stale bytes if the new file is shorter.
                    std::fs::remove_file(&target_abs)?;
                }
                std::fs::copy(&source_abs, &target_abs)?;
                let computed = sha3_file(&target_abs)?;
                if computed != action.expected_sha3 {
                    return Err(RestoreError::TargetDigestMismatch {
                        name: action.name.clone(),
                        stored: action.expected_sha3.clone(),
                        computed,
                    });
                }
                let bytes = std::fs::metadata(&target_abs)?.len();
                restored.push(RestoredComponent {
                    name: action.name.clone(),
                    target_relative: rel_string(&action.target_relative),
                    kind: "file".to_string(),
                    sha3_256: computed,
                    bytes,
                    last_entry_hash: None,
                });
            }
            ActionKind::Tree => {
                // Wipe before write so a forced restore doesn't keep
                // stray files from the previous occupant.
                if target_abs.exists() {
                    std::fs::remove_dir_all(&target_abs)?;
                }
                std::fs::create_dir_all(&target_abs)?;
                copy_dir_recursive(&source_abs, &target_abs)?;
                let (digest, _files, bytes) = sha3_tree(&target_abs)?;
                if digest != action.expected_sha3 {
                    return Err(RestoreError::TargetDigestMismatch {
                        name: action.name.clone(),
                        stored: action.expected_sha3.clone(),
                        computed: digest,
                    });
                }
                let last_entry_hash = if matches!(
                    action.name.as_str(),
                    "api_audit_wal" | "compliance_audit_wal"
                ) {
                    let manifest_component = plan.manifest.component(&action.name);
                    let stored = manifest_component.and_then(|c| c.last_entry_hash.clone());
                    Some(replay_chain(&target_abs, &action.name, &stored)?)
                } else {
                    None
                };
                restored.push(RestoredComponent {
                    name: action.name.clone(),
                    target_relative: rel_string(&action.target_relative),
                    kind: "tree".to_string(),
                    sha3_256: digest,
                    bytes,
                    last_entry_hash,
                });
            }
        }
    }

    // Copy the manifest as `source-manifest.json` so post-restore
    // verification has a witness independent of the backup path.
    let manifest_witness = plan.target_dir.join("source-manifest.json");
    plan.manifest
        .write_to(&manifest_witness)
        .map_err(RestoreError::Manifest)?;

    let report = RestoreReport {
        backup_id: plan.backup_id.clone(),
        backup_dir: plan.backup_dir.display().to_string(),
        target_dir: plan.target_dir.display().to_string(),
        signature_outcome: RestoreSignatureRecord::from(&plan.signature_outcome),
        restored_components: restored,
        warnings: plan.warnings.clone(),
        audit_chain_verified: true,
        forced_overwrite: force && plan.has_blocking_obstacles(),
        completed_at: now_rfc3339(),
        restored_by_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let report_path = plan.target_dir.join("restore-report.json");
    let report_bytes = serde_json::to_vec_pretty(&report)?;
    std::fs::write(&report_path, report_bytes)?;
    Ok(report)
}

// ─── helpers ──────────────────────────────────────────────────────────

fn dir_has_entries(dir: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_some())
}

fn rel_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
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

/// Read every WAL file in `dir` (rotated first, then current.jsonl),
/// run `verify_chain`, and assert the last entry hash matches `stored`
/// when supplied. Returns the observed last hash on success.
fn replay_chain(
    dir: &Path,
    component_name: &str,
    stored: &Option<String>,
) -> Result<String, RestoreError> {
    use crate::audit::{AuditEntry, GENESIS_HASH, verify_chain};
    let mut files: Vec<PathBuf> = Vec::new();
    let rotated = dir.join("rotated");
    if rotated.is_dir() {
        let mut rotated_files: Vec<PathBuf> = std::fs::read_dir(&rotated)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
            .collect();
        rotated_files.sort();
        files.extend(rotated_files);
    }
    let current = dir.join("current.jsonl");
    if current.is_file() {
        files.push(current);
    }
    let mut entries: Vec<AuditEntry> = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)?;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(line)?;
            entries.push(entry);
        }
    }
    if let Err((index, detail)) = verify_chain(&entries) {
        return Err(RestoreError::AuditChainBroken {
            component: component_name.to_string(),
            index,
            detail,
        });
    }
    let observed = entries
        .last()
        .map_or_else(|| GENESIS_HASH.to_string(), |e| e.entry_hash.clone());
    if let Some(want) = stored
        && want != &observed
    {
        return Err(RestoreError::AuditChainLastMismatch {
            component: component_name.to_string(),
            stored: want.clone(),
            computed: observed,
        });
    }
    Ok(observed)
}

fn now_rfc3339() -> String {
    use chrono::Utc;
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BackupManifest, ManifestComponent, ManifestSignatures};

    fn empty_manifest(id: &str) -> BackupManifest {
        BackupManifest {
            backup_id: id.to_string(),
            created_at: "2026-05-23T12:00:00Z".to_string(),
            mai_version: "0.1.0".to_string(),
            git_commit: "abc".to_string(),
            profile: "ship".to_string(),
            host: "test".to_string(),
            migration_version: "test".to_string(),
            components: vec![],
            signatures: ManifestSignatures::default(),
        }
    }

    #[test]
    fn rel_string_normalises_separators() {
        let p = PathBuf::from("audit").join("api").join("current.jsonl");
        let s = rel_string(&p);
        assert!(!s.contains('\\'), "rel_string left a backslash: {s}");
        assert!(s.starts_with("audit/"));
    }

    #[test]
    fn plan_missing_manifest_errors() {
        let td = tempfile::tempdir().unwrap();
        let err = plan_restore(td.path(), td.path(), None, false).unwrap_err();
        assert!(matches!(err, RestoreError::ManifestMissing(_)));
    }

    #[test]
    fn plan_with_empty_manifest_and_empty_target_has_no_obstacles() {
        let backup = tempfile::tempdir().unwrap();
        let manifest = empty_manifest("bk-empty");
        manifest
            .write_to(&backup.path().join("manifest.json"))
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let plan = plan_restore(backup.path(), target.path(), None, false).unwrap();
        assert_eq!(plan.backup_id, "bk-empty");
        assert!(plan.actions.is_empty());
        assert!(plan.obstacles.is_empty());
        assert!(matches!(plan.signature_outcome, VerifyOutcome::Unsigned));
    }

    #[test]
    fn plan_require_signed_rejects_unsigned() {
        let backup = tempfile::tempdir().unwrap();
        let manifest = empty_manifest("bk-need-sig");
        manifest
            .write_to(&backup.path().join("manifest.json"))
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let err = plan_restore(backup.path(), target.path(), None, true).unwrap_err();
        assert!(matches!(err, RestoreError::UnsignedManifest));
    }

    #[test]
    fn component_path_validation_rejects_escape() {
        // AF-11: manifest-supplied component paths must stay within the root.
        for bad in ["../etc/passwd", "/abs/path", "a/../../b", ""] {
            assert!(
                matches!(
                    validate_component_path(bad),
                    Err(RestoreError::UnsafeComponentPath(_))
                ),
                "should reject {bad:?}"
            );
        }
        for ok in ["audit/api", "trust/anchors", "auth/key-hashes.json"] {
            assert!(validate_component_path(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn apply_refuses_when_obstacles_and_force_false() {
        // Manifest lists a single file component; target already has it.
        let backup = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let rel = "auth/key-hashes.json";
        let backup_file = backup.path().join(rel);
        std::fs::create_dir_all(backup_file.parent().unwrap()).unwrap();
        std::fs::write(&backup_file, b"backup-content").unwrap();
        let target_file = target.path().join(rel);
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, b"target-content").unwrap();

        let mut manifest = empty_manifest("bk-conflict");
        manifest.components.push(ManifestComponent {
            name: "auth_key_hashes".to_string(),
            path: rel.to_string(),
            sha3_256: sha3_file(&backup_file).unwrap(),
            bytes: std::fs::metadata(&backup_file).unwrap().len(),
            entry_count: None,
            last_entry_hash: None,
            file_count: None,
        });
        manifest
            .write_to(&backup.path().join("manifest.json"))
            .unwrap();

        let plan = plan_restore(backup.path(), target.path(), None, false).unwrap();
        assert_eq!(plan.obstacles.len(), 1);
        let err = apply_restore(&plan, false).unwrap_err();
        assert!(matches!(err, RestoreError::TargetNotEmpty { .. }));

        // Existing file must NOT have been modified.
        let after = std::fs::read(&target_file).unwrap();
        assert_eq!(after, b"target-content");
    }

    #[test]
    fn apply_with_force_overwrites() {
        let backup = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let rel = "auth/key-hashes.json";
        let backup_file = backup.path().join(rel);
        std::fs::create_dir_all(backup_file.parent().unwrap()).unwrap();
        std::fs::write(&backup_file, b"backup-content").unwrap();
        let target_file = target.path().join(rel);
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, b"target-content").unwrap();

        let mut manifest = empty_manifest("bk-overwrite");
        manifest.components.push(ManifestComponent {
            name: "auth_key_hashes".to_string(),
            path: rel.to_string(),
            sha3_256: sha3_file(&backup_file).unwrap(),
            bytes: std::fs::metadata(&backup_file).unwrap().len(),
            entry_count: None,
            last_entry_hash: None,
            file_count: None,
        });
        manifest
            .write_to(&backup.path().join("manifest.json"))
            .unwrap();

        let plan = plan_restore(backup.path(), target.path(), None, false).unwrap();
        let report = apply_restore(&plan, true).unwrap();
        assert!(report.forced_overwrite);
        let after = std::fs::read(&target_file).unwrap();
        assert_eq!(after, b"backup-content");
        // Report + source manifest were dropped at the root.
        assert!(target.path().join("source-manifest.json").is_file());
        assert!(target.path().join("restore-report.json").is_file());
    }

    #[test]
    fn plan_detects_source_digest_mismatch() {
        // Manifest claims a sha3 that doesn't match the actual file
        // content — proves a corrupt backup never touches the target.
        let backup = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let rel = "auth/key-hashes.json";
        let backup_file = backup.path().join(rel);
        std::fs::create_dir_all(backup_file.parent().unwrap()).unwrap();
        std::fs::write(&backup_file, b"real-content").unwrap();
        let mut manifest = empty_manifest("bk-corrupt");
        manifest.components.push(ManifestComponent {
            name: "auth_key_hashes".to_string(),
            path: rel.to_string(),
            sha3_256: "00".repeat(32),
            bytes: 12,
            entry_count: None,
            last_entry_hash: None,
            file_count: None,
        });
        manifest
            .write_to(&backup.path().join("manifest.json"))
            .unwrap();
        let err = plan_restore(backup.path(), target.path(), None, false).unwrap_err();
        assert!(matches!(err, RestoreError::SourceDigestMismatch { .. }));
        // Target stayed clean.
        assert!(std::fs::read_dir(target.path()).unwrap().next().is_none());
    }
}
