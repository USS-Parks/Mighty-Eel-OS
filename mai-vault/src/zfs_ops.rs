//! Bounded ZFS command execution (plan V5/V6).
//!
//! Every operation here goes through direct argv — never a shell — with
//! validated dataset/snapshot identifiers, a hard timeout, and typed parsing.
//! The [`ZfsRunner`] seam lets unit tests fake the process layer and assert
//! the exact argv; [`SystemZfs`] is the real executor used on a ZFS host.
//!
//! - **V5 — property proof:** [`ZfsOps::dataset_properties`] /
//!   [`ZfsOps::verify_dataset`] query the *actual* dataset and require the
//!   expected encryption, key status, type, mount state, mountpoint, readonly,
//!   and compression. Readiness fails against an ordinary directory
//!   masquerading as ZFS: a missing dataset (or missing `zfs` binary) is a
//!   hard error, never a silent pass.
//! - **V6 — real snapshot operations:** [`ZfsOps::snapshot`],
//!   [`ZfsOps::rollback`], [`ZfsOps::destroy_snapshot`], and
//!   [`ZfsOps::list_snapshots`] execute the actual `zfs` commands. Destroy is
//!   snapshot-only by construction — the argv target is always
//!   `<dataset>@<snapshot>`, so a bare dataset can never be named.
//!
//! Rollback is deliberately un-forced: no `-r`/`-R`/`-f` flags. Rolling back
//! past intermediate snapshots requires the caller to destroy them explicitly
//! first — nothing is deleted implicitly.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tracing::debug;

use mai_core::vault::{SnapshotInfo, VaultError};

/// Hard ceiling on a single `zfs` invocation.
const ZFS_TIMEOUT: Duration = Duration::from_secs(20);

/// ZFS caps full names (dataset or dataset@snapshot) at 255 bytes.
const MAX_NAME: usize = 255;

// ============================================================================
// Identifier validation (V6: validated dataset/snapshot identifiers)
// ============================================================================

fn valid_component_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')
}

/// Validate a ZFS dataset name (`pool/path/to/dataset`).
///
/// Component charset is `[A-Za-z0-9_.:-]`; components may not be empty, `.`
/// or `..`, and may not start with `-` (argv-flag injection). `@` is never
/// valid inside a dataset name here — snapshots are always passed separately.
pub fn validate_dataset(name: &str) -> Result<(), VaultError> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(VaultError::ZfsError(format!(
            "invalid dataset name length {} (must be 1..={MAX_NAME})",
            name.len()
        )));
    }
    for comp in name.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(VaultError::ZfsError(format!(
                "invalid dataset component in {name:?}: empty or relative"
            )));
        }
        if comp.starts_with('-') {
            return Err(VaultError::ZfsError(format!(
                "invalid dataset component in {name:?}: may not start with '-'"
            )));
        }
        if !comp.chars().all(valid_component_char) {
            return Err(VaultError::ZfsError(format!(
                "invalid dataset component in {name:?}: charset is [A-Za-z0-9_.:-]"
            )));
        }
    }
    Ok(())
}

/// Validate a snapshot suffix (the part after `@`). Same charset as dataset
/// components; `/` and `@` are rejected by the charset itself.
pub fn validate_snapshot_name(name: &str) -> Result<(), VaultError> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(VaultError::ZfsError(format!(
            "invalid snapshot name length {} (must be 1..={MAX_NAME})",
            name.len()
        )));
    }
    if name.starts_with('-') {
        return Err(VaultError::ZfsError(format!(
            "invalid snapshot name {name:?}: may not start with '-'"
        )));
    }
    if !name.chars().all(valid_component_char) {
        return Err(VaultError::ZfsError(format!(
            "invalid snapshot name {name:?}: charset is [A-Za-z0-9_.:-]"
        )));
    }
    Ok(())
}

// ============================================================================
// Process seam
// ============================================================================

/// Executes `zfs` with a fixed argv. The seam exists so unit tests can fake
/// the process layer and assert the exact argv the ops construct.
#[async_trait]
pub trait ZfsRunner: Send + Sync {
    /// Run `zfs` with the given argv (no shell). Returns stdout on exit 0.
    async fn zfs(&self, args: &[&str]) -> Result<String, VaultError>;
}

/// Real executor: spawns the system `zfs` binary directly (argv array, no
/// shell), with a hard timeout and stderr capture.
#[derive(Debug, Default)]
pub struct SystemZfs;

impl SystemZfs {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ZfsRunner for SystemZfs {
    async fn zfs(&self, args: &[&str]) -> Result<String, VaultError> {
        debug!(?args, "zfs exec");
        let child = tokio::process::Command::new("zfs")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(ZFS_TIMEOUT, child)
            .await
            .map_err(|_| {
                VaultError::ZfsError(format!(
                    "zfs {} timed out after {ZFS_TIMEOUT:?}",
                    args.first().unwrap_or(&"")
                ))
            })?
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    VaultError::ZfsError(
                        "`zfs` binary not found — this host has no ZFS userspace".into(),
                    )
                } else {
                    VaultError::ZfsError(format!("failed to spawn zfs: {e}"))
                }
            })?;
        if !out.status.success() {
            let mut stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            stderr.truncate(300);
            return Err(VaultError::ZfsError(format!(
                "zfs {} failed ({}): {stderr}",
                args.first().unwrap_or(&""),
                out.status
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

// ============================================================================
// Typed properties (V5)
// ============================================================================

/// Properties read from the actual dataset with `zfs get -H -p`.
#[derive(Debug, Clone, Default)]
pub struct ZfsProperties {
    pub encryption: String,
    pub keystatus: String,
    pub mounted: String,
    pub mountpoint: String,
    pub readonly: String,
    pub compression: String,
    pub dataset_type: String,
    pub quota: u64,
    pub used: u64,
    pub available: u64,
    pub compressratio: f64,
}

/// What a production vault dataset must look like (V5 expectations).
#[derive(Debug, Clone)]
pub struct DatasetExpectations {
    /// Require native encryption on and its key loaded (`keystatus=available`).
    pub require_encryption: bool,
    /// Pin the mountpoint to this exact path, if set.
    pub mountpoint: Option<String>,
    /// Require `compression` != `off`.
    pub require_compression: bool,
    /// Require `readonly` == `off` (the vault writes model packages).
    pub require_writable: bool,
}

impl Default for DatasetExpectations {
    fn default() -> Self {
        Self {
            require_encryption: true,
            mountpoint: None,
            require_compression: true,
            require_writable: true,
        }
    }
}

// ============================================================================
// Operations
// ============================================================================

/// Bounded ZFS operations over a [`ZfsRunner`].
pub struct ZfsOps {
    runner: Box<dyn ZfsRunner>,
}

impl ZfsOps {
    /// Ops backed by the system `zfs` binary.
    pub fn system() -> Self {
        Self::with_runner(Box::new(SystemZfs::new()))
    }

    /// Ops backed by an explicit runner (tests use a fake).
    pub fn with_runner(runner: Box<dyn ZfsRunner>) -> Self {
        Self { runner }
    }

    fn snap_target(dataset: &str, snapshot: &str) -> Result<String, VaultError> {
        validate_dataset(dataset)?;
        validate_snapshot_name(snapshot)?;
        let target = format!("{dataset}@{snapshot}");
        if target.len() > MAX_NAME {
            return Err(VaultError::ZfsError(format!(
                "snapshot target too long: {} bytes",
                target.len()
            )));
        }
        Ok(target)
    }

    /// V5: read the actual dataset properties. A missing dataset or missing
    /// `zfs` binary is a hard error — a plain directory cannot answer this.
    pub async fn dataset_properties(&self, dataset: &str) -> Result<ZfsProperties, VaultError> {
        validate_dataset(dataset)?;
        let out = self
            .runner
            .zfs(&[
                "get",
                "-H",
                "-p",
                "-o",
                "property,value",
                "encryption,keystatus,mounted,mountpoint,readonly,compression,type,quota,used,available,compressratio",
                dataset,
            ])
            .await?;
        let mut props = ZfsProperties::default();
        for line in out.lines() {
            let Some((prop, value)) = line.split_once('\t') else {
                return Err(VaultError::ZfsError(format!(
                    "unparseable `zfs get` line: {line:?}"
                )));
            };
            let value = value.trim();
            match prop {
                "encryption" => props.encryption = value.to_string(),
                "keystatus" => props.keystatus = value.to_string(),
                "mounted" => props.mounted = value.to_string(),
                "mountpoint" => props.mountpoint = value.to_string(),
                "readonly" => props.readonly = value.to_string(),
                "compression" => props.compression = value.to_string(),
                "type" => props.dataset_type = value.to_string(),
                "quota" => props.quota = value.parse().unwrap_or(0),
                "used" => props.used = value.parse().unwrap_or(0),
                "available" => props.available = value.parse().unwrap_or(0),
                "compressratio" => {
                    props.compressratio = value.trim_end_matches('x').parse().unwrap_or(1.0);
                }
                _ => {}
            }
        }
        if props.dataset_type.is_empty() {
            return Err(VaultError::ZfsError(format!(
                "dataset {dataset} returned no `type` property"
            )));
        }
        Ok(props)
    }

    /// V5: require the dataset to match expectations, and prove snapshot
    /// capability by listing (an empty list is fine; an error is not).
    /// Every mismatch fails with the offending property named.
    pub async fn verify_dataset(
        &self,
        dataset: &str,
        expect: &DatasetExpectations,
    ) -> Result<ZfsProperties, VaultError> {
        let props = self.dataset_properties(dataset).await?;
        let fail = |what: &str, got: &str| {
            Err(VaultError::ZfsError(format!(
                "dataset {dataset} failed readiness: {what} (got {got:?})"
            )))
        };
        if props.dataset_type != "filesystem" {
            return fail("type must be `filesystem`", &props.dataset_type);
        }
        if expect.require_encryption {
            if props.encryption == "off" || props.encryption.is_empty() {
                return fail("native encryption must be on", &props.encryption);
            }
            if props.keystatus != "available" {
                return fail("encryption key must be loaded", &props.keystatus);
            }
        }
        if props.mounted != "yes" {
            return fail("dataset must be mounted", &props.mounted);
        }
        if let Some(want) = &expect.mountpoint
            && &props.mountpoint != want
        {
            return fail(&format!("mountpoint must be {want}"), &props.mountpoint);
        }
        if expect.require_writable && props.readonly != "off" {
            return fail("dataset must be writable", &props.readonly);
        }
        if expect.require_compression && props.compression == "off" {
            return fail("compression must be enabled", &props.compression);
        }
        // Snapshot capability: the dataset must answer a snapshot listing.
        self.list_snapshots(dataset).await?;
        Ok(props)
    }

    /// V6: create `<dataset>@<snapshot>`.
    pub async fn snapshot(&self, dataset: &str, snapshot: &str) -> Result<(), VaultError> {
        let target = Self::snap_target(dataset, snapshot)?;
        self.runner.zfs(&["snapshot", &target]).await?;
        Ok(())
    }

    /// V6: roll the dataset back to `<dataset>@<snapshot>`. Un-forced: fails
    /// if more recent snapshots exist (destroy them explicitly first).
    pub async fn rollback(&self, dataset: &str, snapshot: &str) -> Result<(), VaultError> {
        let target = Self::snap_target(dataset, snapshot)?;
        self.runner.zfs(&["rollback", &target]).await?;
        Ok(())
    }

    /// V6: destroy `<dataset>@<snapshot>` — snapshot-only by construction:
    /// the argv target always contains `@`, so a bare dataset can never be
    /// destroyed through this path.
    pub async fn destroy_snapshot(&self, dataset: &str, snapshot: &str) -> Result<(), VaultError> {
        let target = Self::snap_target(dataset, snapshot)?;
        debug_assert!(target.contains('@'));
        self.runner.zfs(&["destroy", &target]).await?;
        Ok(())
    }

    /// V6: list the dataset's own snapshots (creation-ordered), with real
    /// creation times and referenced bytes.
    pub async fn list_snapshots(&self, dataset: &str) -> Result<Vec<SnapshotInfo>, VaultError> {
        validate_dataset(dataset)?;
        let out = self
            .runner
            .zfs(&[
                "list",
                "-H",
                "-p",
                "-t",
                "snapshot",
                "-o",
                "name,creation,referenced",
                "-s",
                "creation",
                "-d",
                "1",
                dataset,
            ])
            .await?;
        let mut snaps = Vec::new();
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            let mut cols = line.split('\t');
            let (Some(name), Some(creation), Some(referenced)) =
                (cols.next(), cols.next(), cols.next())
            else {
                return Err(VaultError::ZfsError(format!(
                    "unparseable `zfs list` line: {line:?}"
                )));
            };
            let Some((_, snap)) = name.split_once('@') else {
                return Err(VaultError::ZfsError(format!(
                    "snapshot listing returned a non-snapshot name: {name:?}"
                )));
            };
            snaps.push(SnapshotInfo {
                name: snap.to_string(),
                created_at: creation.trim().parse().unwrap_or(0),
                referenced_bytes: referenced.trim().parse().unwrap_or(0),
                reason: String::new(),
            });
        }
        Ok(snaps)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Fake runner: records every argv, replays queued responses in order.
    struct FakeZfs {
        calls: Mutex<Vec<Vec<String>>>,
        replies: Mutex<Vec<Result<String, VaultError>>>,
    }

    impl FakeZfs {
        fn new(replies: Vec<Result<String, VaultError>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                replies: Mutex::new(replies),
            }
        }
    }

    #[async_trait]
    impl ZfsRunner for FakeZfs {
        async fn zfs(&self, args: &[&str]) -> Result<String, VaultError> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(ToString::to_string).collect());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Ok(String::new());
            }
            replies.remove(0)
        }
    }

    fn ops_with(replies: Vec<Result<String, VaultError>>) -> (ZfsOps, std::sync::Arc<FakeZfs>) {
        // Keep a second handle to the fake so calls can be asserted after use.
        struct Shared(std::sync::Arc<FakeZfs>);
        #[async_trait]
        impl ZfsRunner for Shared {
            async fn zfs(&self, args: &[&str]) -> Result<String, VaultError> {
                self.0.zfs(args).await
            }
        }
        let fake = std::sync::Arc::new(FakeZfs::new(replies));
        (ZfsOps::with_runner(Box::new(Shared(fake.clone()))), fake)
    }

    fn good_props_output() -> String {
        [
            "encryption\taes-256-gcm",
            "keystatus\tavailable",
            "mounted\tyes",
            "mountpoint\t/vault/models",
            "readonly\toff",
            "compression\tlz4",
            "type\tfilesystem",
            "quota\t0",
            "used\t1024",
            "available\t1073741824",
            "compressratio\t1.50",
        ]
        .join("\n")
    }

    // -- identifier validation --------------------------------------------

    #[test]
    fn validate_dataset_accepts_normal_names() {
        for ok in ["im-vault/models", "tank", "a-1._:2/b.c", "pool/a/b/c"] {
            assert!(validate_dataset(ok).is_ok(), "{ok} should be valid");
        }
    }

    #[test]
    fn validate_dataset_rejects_injection_shapes() {
        for bad in [
            "",
            "a b",
            "a;b",
            "-flag/x",
            "pool/-rf",
            "a/../b",
            "a/./b",
            "a//b",
            "a@b",
            "a\nb",
            "a/$(reboot)",
        ] {
            assert!(validate_dataset(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_dataset(&"x".repeat(300)).is_err());
    }

    #[test]
    fn validate_snapshot_rejects_injection_shapes() {
        for bad in ["", "-r", "a/b", "a@b", "a b", "a;b"] {
            assert!(
                validate_snapshot_name(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_snapshot_name("mai-snap-20260707-101500").is_ok());
    }

    // -- argv exactness (V6: direct argv, bounded flags) -------------------

    #[tokio::test]
    async fn snapshot_ops_build_exact_bounded_argv() {
        let (ops, fake) = ops_with(vec![
            Ok(String::new()),
            Ok(String::new()),
            Ok(String::new()),
        ]);
        ops.snapshot("im-vault/models", "s1").await.unwrap();
        ops.rollback("im-vault/models", "s1").await.unwrap();
        ops.destroy_snapshot("im-vault/models", "s1").await.unwrap();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["snapshot", "im-vault/models@s1"]);
        assert_eq!(calls[1], vec!["rollback", "im-vault/models@s1"]);
        assert_eq!(calls[2], vec!["destroy", "im-vault/models@s1"]);
        // Destroy target always carries '@' — a bare dataset is unreachable.
        assert!(calls[2][1].contains('@'));
    }

    #[tokio::test]
    async fn snapshot_ops_refuse_invalid_identifiers_before_exec() {
        let (ops, fake) = ops_with(vec![]);
        assert!(ops.snapshot("a;b", "s1").await.is_err());
        assert!(ops.snapshot("im-vault/models", "-r").await.is_err());
        assert!(
            ops.destroy_snapshot("im-vault/models", "s1/..")
                .await
                .is_err()
        );
        assert!(
            fake.calls.lock().unwrap().is_empty(),
            "no zfs process may run for invalid identifiers"
        );
    }

    // -- listing ------------------------------------------------------------

    #[tokio::test]
    async fn list_snapshots_parses_real_output_shape() {
        let out =
            "im-vault/models@snap-a\t1751882000\t2048\nim-vault/models@snap-b\t1751882100\t4096\n";
        let (ops, fake) = ops_with(vec![Ok(out.into())]);
        let snaps = ops.list_snapshots("im-vault/models").await.unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].name, "snap-a");
        assert_eq!(snaps[0].created_at, 1_751_882_000);
        assert_eq!(snaps[1].referenced_bytes, 4096);
        let calls = fake.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            vec![
                "list",
                "-H",
                "-p",
                "-t",
                "snapshot",
                "-o",
                "name,creation,referenced",
                "-s",
                "creation",
                "-d",
                "1",
                "im-vault/models"
            ]
        );
    }

    #[tokio::test]
    async fn list_snapshots_empty_and_malformed() {
        let (ops, _) = ops_with(vec![Ok(String::new())]);
        assert!(ops.list_snapshots("t/d").await.unwrap().is_empty());
        let (ops, _) = ops_with(vec![Ok("garbage-without-tabs".into())]);
        assert!(ops.list_snapshots("t/d").await.is_err());
    }

    // -- V5 property proof ---------------------------------------------------

    #[tokio::test]
    async fn verify_dataset_passes_on_expected_properties() {
        let (ops, _) = ops_with(vec![Ok(good_props_output()), Ok(String::new())]);
        let props = ops
            .verify_dataset(
                "im-vault/models",
                &DatasetExpectations {
                    mountpoint: Some("/vault/models".into()),
                    ..DatasetExpectations::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(props.encryption, "aes-256-gcm");
        assert_eq!(props.used, 1024);
        assert!((props.compressratio - 1.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn verify_dataset_fails_each_negative_control() {
        // (replace property line, expected error fragment)
        let cases = [
            ("encryption\taes-256-gcm", "encryption\toff", "encryption"),
            ("keystatus\tavailable", "keystatus\tunavailable", "key"),
            ("mounted\tyes", "mounted\tno", "mounted"),
            ("readonly\toff", "readonly\ton", "writable"),
            ("compression\tlz4", "compression\toff", "compression"),
            ("type\tfilesystem", "type\tvolume", "filesystem"),
        ];
        for (from, to, needle) in cases {
            let out = good_props_output().replace(from, to);
            let (ops, _) = ops_with(vec![Ok(out)]);
            let err = ops
                .verify_dataset("im-vault/models", &DatasetExpectations::default())
                .await
                .expect_err(needle);
            assert!(
                err.to_string().contains(needle),
                "error for {to:?} should mention {needle}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn verify_dataset_pins_mountpoint() {
        let (ops, _) = ops_with(vec![Ok(good_props_output())]);
        let err = ops
            .verify_dataset(
                "im-vault/models",
                &DatasetExpectations {
                    mountpoint: Some("/elsewhere".into()),
                    ..DatasetExpectations::default()
                },
            )
            .await
            .expect_err("wrong mountpoint");
        assert!(err.to_string().contains("mountpoint"));
    }

    /// The "ordinary directory masquerading as ZFS" control: on a host with
    /// no `zfs` binary the spawn fails; on a ZFS host the dataset does not
    /// exist. Either way this must be a hard error, never a pass.
    #[tokio::test]
    async fn system_zfs_fails_closed_without_a_real_dataset() {
        let ops = ZfsOps::system();
        let err = ops
            .dataset_properties("mai-vault-zfsops-negative-fixture")
            .await
            .expect_err("plain host must not satisfy the property proof");
        assert!(matches!(err, VaultError::ZfsError(_)));
    }
}
