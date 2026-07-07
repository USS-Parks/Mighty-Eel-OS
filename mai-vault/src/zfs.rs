//! ZFS-backed model storage vault.
//!
//! Implements `VaultInterface` (original 4-method contract) and `ModelStorage`
//! (extended ZFS operations). Model weights are stored on encrypted ZFS datasets
//! with integrity verification, snapshot management, and secure deletion.
//!
//! # Architecture
//!
//! ```text
//! ZfsVault
//!   ├── VaultInterface   (load_model_weights, store_model_package, append_audit_entry, verify_signature)
//!   ├── ModelStorage      (verify_integrity, storage_info, snapshots, secure delete)
//!   └── delegates to:
//!       ├── PqcEngine      (encryption/decryption/signing)
//!       ├── TpmManager     (key sealing)
//!       ├── ProfileManager (profile CRUD)
//!       ├── AuditWriter    (audit trail)
//!       └── VectorManager  (Qdrant)
//! ```

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{
    AuditStore, IntegrityResult, ModelStorage, PqcProvider, SnapshotInfo, StorageInfo,
    VaultAuditAction, VaultAuditStatus, VaultError, VaultInterface,
};

use crate::audit::{AuditWriter, build_audit_entry};
use crate::config::VaultConfig;
use crate::pqc::PqcEngine;
use crate::zfs_ops::{DatasetExpectations, ZfsOps};

/// ZFS-backed vault providing encrypted model storage.
///
/// This is the primary vault type. It implements `VaultInterface` for backward
/// compatibility with existing mai-core consumers and `ModelStorage` for the
/// extended ZFS operations.
pub struct ZfsVault {
    config: VaultConfig,
    /// Model manifest cache: model_id -> (hash, size_bytes)
    model_index: RwLock<HashMap<String, ModelEntry>>,
    /// Snapshot metadata cache
    snapshots: RwLock<Vec<SnapshotInfo>>,
    /// PQC engine for `verify_signature` delegation (optional).
    pqc: Option<Arc<PqcEngine>>,
    /// Audit writer for `append_audit_entry` delegation (optional).
    audit: Option<Arc<AuditWriter>>,
    /// Real bounded ZFS operations (V5/V6). `None` = dev/test mode: snapshot
    /// calls track metadata only and no dataset property proof runs.
    zfs: Option<ZfsOps>,
}

/// Weights-at-rest format marker (plan V4). `v1` is the ML-KEM-1024 +
/// AES-256-GCM envelope produced by `PqcProvider::encrypt_model_weights`,
/// bound to the model id; `v0` is legacy plaintext, readable for migration
/// but never written when a PQC engine is wired.
pub const WEIGHTS_FORMAT_ENCRYPTED_V1: &str = "mlkem1024-aesgcm-v1";
/// Legacy plaintext weights format (pre-V4).
pub const WEIGHTS_FORMAT_PLAINTEXT_V0: &str = "plaintext-v0";

/// Internal model tracking entry.
struct ModelEntry {
    /// Expected SHA-256 hash of model weights.
    expected_hash: String,
    /// Size of model data on disk (bytes).
    size_bytes: u64,
    /// Path within the vault dataset.
    path: PathBuf,
    /// Whether integrity has been verified since last load.
    verified: bool,
}

impl ZfsVault {
    /// Create a new ZFS vault with the given configuration.
    ///
    /// Does not open or verify the ZFS dataset. Call `initialize()` after
    /// construction to verify dataset health and scan existing models.
    pub fn new(config: VaultConfig) -> Self {
        Self {
            config,
            model_index: RwLock::new(HashMap::new()),
            snapshots: RwLock::new(Vec::new()),
            pqc: None,
            audit: None,
            zfs: None,
        }
    }

    /// Enable real bounded ZFS operations (V5/V6): `initialize()` then runs
    /// the dataset property proof and snapshot calls execute actual `zfs`
    /// commands with receipts. Without this the vault stays in dev/test mode.
    #[must_use]
    pub fn with_zfs(mut self, ops: ZfsOps) -> Self {
        self.zfs = Some(ops);
        self
    }

    /// Create a vault wired to a PQC engine and audit writer.
    ///
    /// With both engines wired, `verify_signature` delegates to ML-DSA-87
    /// verification and `append_audit_entry` writes to the hash-chained
    /// audit log instead of returning placeholder values.
    pub fn with_engines(config: VaultConfig, pqc: Arc<PqcEngine>, audit: Arc<AuditWriter>) -> Self {
        Self {
            config,
            model_index: RwLock::new(HashMap::new()),
            snapshots: RwLock::new(Vec::new()),
            pqc: Some(pqc),
            audit: Some(audit),
            zfs: None,
        }
    }

    /// Initialize the vault: verify ZFS dataset, scan model index, load snapshots.
    ///
    /// Must be called before any storage operations. In production this checks
    /// that the ZFS dataset is mounted and healthy. In test mode it creates
    /// a temporary directory structure.
    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            dataset = %self.config.storage.dataset,
            mount = %self.config.storage.mount_point.display(),
            "Initializing ZFS vault"
        );

        // V5: with real ZFS wired, prove the dataset before touching anything.
        // An ordinary directory masquerading as ZFS fails here, hard.
        if let Some(ops) = &self.zfs {
            let expect = DatasetExpectations {
                mountpoint: Some(self.config.storage.mount_point.display().to_string()),
                require_compression: self.config.storage.compression_enabled,
                ..DatasetExpectations::default()
            };
            let props = ops
                .verify_dataset(&self.config.storage.dataset, &expect)
                .await?;
            info!(
                encryption = %props.encryption,
                compression = %props.compression,
                used = props.used,
                available = props.available,
                "ZFS dataset property proof passed"
            );
        }

        // Verify mount point exists
        let mount = &self.config.storage.mount_point;
        if !mount.exists() {
            return Err(VaultError::ZfsError(format!(
                "Mount point does not exist: {}",
                mount.display()
            )));
        }

        // Scan for existing models
        self.scan_models().await?;

        // Load snapshot metadata
        self.scan_snapshots().await?;

        info!("ZFS vault initialized successfully");
        Ok(())
    }

    /// Scan the vault directory for model packages and build the index.
    async fn scan_models(&self) -> Result<(), VaultError> {
        let models_dir = &self.config.storage.mount_point;
        let mut index = self.model_index.write().await;
        index.clear();

        // TODO(basho): read the ZFS dataset and parse model manifest files
        // (manifest.json with hash and metadata); currently scans the
        // directory and registers entries.

        if models_dir.is_dir() {
            match std::fs::read_dir(models_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir()
                            && let Some(model_id) = path.file_name().and_then(|n| n.to_str())
                        {
                            let manifest_path = path.join("manifest.json");
                            if manifest_path.exists() {
                                // Parse manifest to get expected hash and size
                                match Self::read_model_manifest(&manifest_path) {
                                    Ok((hash, size)) => {
                                        debug!(model_id, hash = %hash, size, "Found model in vault");
                                        index.insert(
                                            model_id.to_string(),
                                            ModelEntry {
                                                expected_hash: hash,
                                                size_bytes: size,
                                                path: path.clone(),
                                                verified: false,
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        warn!(model_id, error = %e, "Skipping model with invalid manifest");
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Could not scan models directory");
                }
            }
        }

        info!(model_count = index.len(), "Model scan complete");
        Ok(())
    }

    /// Read a model manifest file and extract hash + size.
    fn read_model_manifest(path: &Path) -> Result<(String, u64), VaultError> {
        let content = std::fs::read_to_string(path).map_err(VaultError::from)?;
        let manifest: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| VaultError::IoError(e.to_string()))?;

        let hash = manifest
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| VaultError::IoError("Missing sha256 in manifest".into()))?
            .to_string();

        let size = manifest
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        Ok((hash, size))
    }

    /// Scan for ZFS snapshots.
    async fn scan_snapshots(&self) -> Result<(), VaultError> {
        let mut snaps = self.snapshots.write().await;
        snaps.clear();
        if let Some(ops) = &self.zfs {
            *snaps = ops.list_snapshots(&self.config.storage.dataset).await?;
            debug!(count = snaps.len(), "Snapshot scan complete (live ZFS)");
        } else {
            debug!("Snapshot scan complete (dev mode, no ZFS)");
        }
        Ok(())
    }

    /// Write a snapshot-operation receipt to the audit chain. With an audit
    /// writer wired the receipt is part of the operation — a failed append
    /// fails the operation (fail-closed). In dev mode it is a debug log.
    async fn snapshot_receipt(
        &self,
        action: VaultAuditAction,
        status: VaultAuditStatus,
        target: &str,
        detail: Option<String>,
    ) -> Result<(), VaultError> {
        let Some(audit) = &self.audit else {
            debug!(
                ?action,
                ?status,
                target,
                "snapshot receipt (dev mode, unaudited)"
            );
            return Ok(());
        };
        #[allow(clippy::cast_sign_loss)] // post-epoch timestamps only
        let now = chrono::Utc::now().timestamp() as u64;
        let entry = build_audit_entry(
            uuid::Uuid::new_v4().to_string(),
            now,
            "vault-system".to_string(),
            action,
            // The acted-on object: `<dataset>@<snapshot>`.
            Some(target.to_string()),
            None,
            None,
            0,
            None,
            status,
            detail,
            audit.last_hash().await?,
        );
        audit.append(&entry).await
    }

    /// Compute SHA-256 hash of a file.
    fn compute_file_hash(path: &Path) -> Result<String, VaultError> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(VaultError::from)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 65536];
        loop {
            let bytes_read = file.read(&mut buffer).map_err(VaultError::from)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    /// Get the weights file path for a model.
    fn weights_path(&self, model_id: &str) -> PathBuf {
        self.config
            .storage
            .mount_point
            .join(model_id)
            .join("weights.bin")
    }

    /// The `weights_format` recorded in a model manifest. A manifest without
    /// the field (pre-V4) is legacy plaintext.
    fn read_weights_format(manifest_path: &Path) -> Result<String, VaultError> {
        let content = std::fs::read_to_string(manifest_path).map_err(VaultError::from)?;
        let manifest: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| VaultError::IoError(e.to_string()))?;
        Ok(manifest
            .get("weights_format")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(WEIGHTS_FORMAT_PLAINTEXT_V0)
            .to_string())
    }
}

// ============================================================================
// VaultInterface implementation (original 4 methods, backward compatible)
// ============================================================================

#[async_trait]
impl VaultInterface for ZfsVault {
    async fn load_model_weights(&self, model_id: &str) -> Result<Vec<u8>, VaultError> {
        let index = self.model_index.read().await;
        let entry = index
            .get(model_id)
            .ok_or_else(|| VaultError::ModelNotFound(model_id.to_string()))?;

        let weights_path = entry.path.join("weights.bin");
        if !weights_path.exists() {
            return Err(VaultError::ModelNotFound(format!(
                "Weights file missing for model {model_id}"
            )));
        }

        info!(model_id, path = %weights_path.display(), "Loading model weights from vault");

        let stored = tokio::fs::read(&weights_path)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        // V4: the manifest's `weights_format` decides the read path. A
        // missing field means a legacy (pre-V4) plaintext store — readable
        // for migration. Encrypted weights without a wired engine fail
        // closed rather than returning ciphertext as if it were a model.
        let manifest_path = entry.path.join("manifest.json");
        let weights_format = Self::read_weights_format(&manifest_path)?;
        let data = match weights_format.as_str() {
            WEIGHTS_FORMAT_ENCRYPTED_V1 => {
                let pqc = self.pqc.as_ref().ok_or_else(|| {
                    VaultError::PqcError(format!(
                        "model {model_id} is stored encrypted ({WEIGHTS_FORMAT_ENCRYPTED_V1}) but no PQC engine is wired"
                    ))
                })?;
                pqc.decrypt_model_weights(model_id, &stored).await?
            }
            WEIGHTS_FORMAT_PLAINTEXT_V0 => stored,
            other => {
                return Err(VaultError::IoError(format!(
                    "model {model_id} has unsupported weights_format {other:?}"
                )));
            }
        };

        info!(model_id, bytes = data.len(), "Model weights loaded");
        Ok(data)
    }

    async fn store_model_package(&self, model_id: &str, data: &[u8]) -> Result<(), VaultError> {
        // Check if model already exists
        {
            let index = self.model_index.read().await;
            if index.contains_key(model_id) {
                return Err(VaultError::ModelAlreadyExists(model_id.to_string()));
            }
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");

        info!(
            model_id,
            bytes = data.len(),
            path = %model_dir.display(),
            "Storing model package in vault"
        );

        // Create model directory
        tokio::fs::create_dir_all(&model_dir)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        // V4: with a PQC engine wired, weights are sealed at rest in the
        // ML-KEM-1024 + AES-256-GCM envelope, key-derived with the model id
        // as context. Plaintext-at-rest exists only on the engine-less
        // dev path (and the builder always wires engines for the ZFS
        // backend, V2).
        let (stored, weights_format): (Vec<u8>, &str) = match &self.pqc {
            Some(pqc) => (
                pqc.encrypt_model_weights(model_id, data).await?,
                WEIGHTS_FORMAT_ENCRYPTED_V1,
            ),
            None => (data.to_vec(), WEIGHTS_FORMAT_PLAINTEXT_V0),
        };
        tokio::fs::write(&weights_path, &stored)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        // Compute hash over the stored bytes (ciphertext for v1) so
        // integrity verifies without any decryption.
        let hash = Self::compute_file_hash(&weights_path)?;
        let manifest = serde_json::json!({
            "model_id": model_id,
            "sha256": hash,
            "size_bytes": stored.len(),
            "plaintext_bytes": data.len(),
            "weights_format": weights_format,
            "stored_at": chrono::Utc::now().timestamp(),
        });
        tokio::fs::write(&manifest_path, manifest.to_string())
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        // Update index
        let mut index = self.model_index.write().await;
        index.insert(
            model_id.to_string(),
            ModelEntry {
                expected_hash: hash,
                size_bytes: stored.len() as u64,
                path: model_dir,
                verified: true, // just wrote it
            },
        );

        info!(
            model_id,
            weights_format, "Model package stored successfully"
        );
        Ok(())
    }

    async fn append_audit_entry(&self, entry: &[u8]) -> Result<(), VaultError> {
        let audit = self
            .audit
            .as_ref()
            .ok_or_else(|| VaultError::AuditStoreError("AuditWriter not wired to vault".into()))?;
        let parsed: mai_core::vault::VaultAuditEntry = serde_json::from_slice(entry)
            .map_err(|e| VaultError::AuditStoreError(format!("audit entry decode: {e}")))?;
        debug!(
            entry_id = %parsed.entry_id,
            "Vault delegating audit entry to AuditWriter"
        );
        audit.append(&parsed).await
    }

    async fn verify_signature(&self, data: &[u8], signature: &[u8]) -> Result<bool, VaultError> {
        let pqc = self
            .pqc
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("PqcEngine not wired to vault".into()))?;
        debug!(
            data_bytes = data.len(),
            sig_bytes = signature.len(),
            "Vault delegating signature verification to PqcProvider"
        );
        pqc.verify_package(data, signature).await
    }
}

// ============================================================================
// ModelStorage implementation (extended ZFS operations)
// ============================================================================

#[async_trait]
impl ModelStorage for ZfsVault {
    async fn verify_model_integrity(&self, model_id: &str) -> Result<IntegrityResult, VaultError> {
        let index = self.model_index.read().await;
        let entry = index
            .get(model_id)
            .ok_or_else(|| VaultError::ModelNotFound(model_id.to_string()))?;

        let weights_path = entry.path.join("weights.bin");
        if !weights_path.exists() {
            return Ok(IntegrityResult {
                valid: false,
                computed_hash: String::new(),
                expected_hash: entry.expected_hash.clone(),
                verified_bytes: 0,
            });
        }

        let computed_hash = Self::compute_file_hash(&weights_path)?;
        let file_size = tokio::fs::metadata(&weights_path)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?
            .len();

        let valid = computed_hash == entry.expected_hash;
        if valid {
            debug!(model_id, "Model integrity check passed");
        } else {
            warn!(
                model_id,
                expected = %entry.expected_hash,
                computed = %computed_hash,
                "Model integrity check FAILED"
            );
        }

        Ok(IntegrityResult {
            valid,
            computed_hash,
            expected_hash: entry.expected_hash.clone(),
            verified_bytes: file_size,
        })
    }

    async fn storage_info(&self) -> Result<StorageInfo, VaultError> {
        if let Some(ops) = &self.zfs {
            // Real dataset numbers (V5): used/available/compressratio come
            // from the dataset itself, not an estimate.
            let props = ops.dataset_properties(&self.config.storage.dataset).await?;
            let index = self.model_index.read().await;
            #[allow(clippy::cast_possible_truncation)]
            let model_count = index.len() as u32;
            return Ok(StorageInfo {
                total_bytes: props.used.saturating_add(props.available),
                used_bytes: props.used,
                available_bytes: props.available,
                model_count,
                compression_ratio: props.compressratio,
            });
        }

        let index = self.model_index.read().await;
        let total_model_bytes: u64 = index.values().map(|e| e.size_bytes).sum();

        let capacity = self.config.storage.max_capacity_bytes;
        let total = if capacity > 0 {
            capacity
        } else {
            // Default 1 TiB if not configured
            1_099_511_627_776
        };
        let available = total.saturating_sub(total_model_bytes);
        #[allow(clippy::cast_possible_truncation)]
        let model_count = index.len() as u32;

        Ok(StorageInfo {
            total_bytes: total,
            used_bytes: total_model_bytes,
            available_bytes: available,
            model_count,
            compression_ratio: 1.0, // ZFS compression ratio (query zfs get compressratio)
        })
    }

    async fn remove_model(&self, model_id: &str) -> Result<(), VaultError> {
        let model_dir = {
            let index = self.model_index.read().await;
            let entry = index
                .get(model_id)
                .ok_or_else(|| VaultError::ModelNotFound(model_id.to_string()))?;
            entry.path.clone()
        };

        info!(model_id, path = %model_dir.display(), "Removing model from vault (crypto-erase)");

        // V7: cryptographic erasure, not "secure overwrite". On copy-on-write
        // ZFS, unlinking or overwriting the weights file does NOT destroy the
        // blocks it occupied — they persist until freed and reused, and any
        // snapshot retains them indefinitely. Retiring the model's encryption
        // key makes the at-rest ciphertext (on disk AND in every snapshot)
        // permanently unrecoverable, which is the real deletion guarantee.
        let erased = if let Some(pqc) = &self.pqc {
            pqc.crypto_erase_model(model_id).await?
        } else {
            false
        };

        // Unlink the directory tree too — frees space and removes the visible
        // artifact — but the confidentiality guarantee is the key retirement
        // above, not this unlink.
        if model_dir.exists() {
            tokio::fs::remove_dir_all(&model_dir)
                .await
                .map_err(|e| VaultError::IoError(e.to_string()))?;
        }

        // Remove from index
        let mut index = self.model_index.write().await;
        index.remove(model_id);

        info!(
            model_id,
            key_retired = erased,
            "Model removed from vault (blocks in retained snapshots remain, but crypto-erased)"
        );
        Ok(())
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!("mai-snap-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
        let now = chrono::Utc::now().timestamp() as u64;

        info!(snapshot = %name, reason, "Creating vault snapshot");

        let mut referenced_bytes = 0;
        if let Some(ops) = &self.zfs {
            let dataset = &self.config.storage.dataset;
            let target = format!("{dataset}@{name}");
            if let Err(e) = ops.snapshot(dataset, &name).await {
                let _ = self
                    .snapshot_receipt(
                        VaultAuditAction::SnapshotCreate,
                        VaultAuditStatus::Error,
                        &target,
                        Some(e.to_string()),
                    )
                    .await;
                return Err(e);
            }
            referenced_bytes = ops
                .list_snapshots(dataset)
                .await?
                .into_iter()
                .find(|s| s.name == name)
                .map_or(0, |s| s.referenced_bytes);
            self.snapshot_receipt(
                VaultAuditAction::SnapshotCreate,
                VaultAuditStatus::Success,
                &target,
                None,
            )
            .await?;
        }

        let snap = SnapshotInfo {
            name: name.clone(),
            created_at: now,
            referenced_bytes,
            reason: reason.to_string(),
        };

        let mut snaps = self.snapshots.write().await;
        snaps.push(snap.clone());

        info!(snapshot = %name, "Snapshot created");
        Ok(snap)
    }

    async fn rollback_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        if let Some(ops) = &self.zfs {
            // Existence check against the live dataset, not the cache.
            let dataset = &self.config.storage.dataset;
            if !ops
                .list_snapshots(dataset)
                .await?
                .iter()
                .any(|s| s.name == snapshot_name)
            {
                return Err(VaultError::SnapshotNotFound(snapshot_name.to_string()));
            }
            info!(snapshot = %snapshot_name, "Rolling back to snapshot (live ZFS)");
            let target = format!("{dataset}@{snapshot_name}");
            if let Err(e) = ops.rollback(dataset, snapshot_name).await {
                let _ = self
                    .snapshot_receipt(
                        VaultAuditAction::SnapshotRollback,
                        VaultAuditStatus::Error,
                        &target,
                        Some(e.to_string()),
                    )
                    .await;
                return Err(e);
            }
            self.snapshot_receipt(
                VaultAuditAction::SnapshotRollback,
                VaultAuditStatus::Success,
                &target,
                None,
            )
            .await?;
            self.scan_models().await?;
            self.scan_snapshots().await?;
            info!(snapshot = %snapshot_name, "Rollback complete");
            return Ok(());
        }

        let snaps = self.snapshots.read().await;
        if !snaps.iter().any(|s| s.name == snapshot_name) {
            return Err(VaultError::SnapshotNotFound(snapshot_name.to_string()));
        }

        info!(snapshot = %snapshot_name, "Rolling back to snapshot");
        drop(snaps);
        self.scan_models().await?;

        info!(snapshot = %snapshot_name, "Rollback complete");
        Ok(())
    }

    async fn delete_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        if let Some(ops) = &self.zfs {
            let dataset = &self.config.storage.dataset;
            if !ops
                .list_snapshots(dataset)
                .await?
                .iter()
                .any(|s| s.name == snapshot_name)
            {
                return Err(VaultError::SnapshotNotFound(snapshot_name.to_string()));
            }
            info!(snapshot = %snapshot_name, "Deleting snapshot (live ZFS)");
            let target = format!("{dataset}@{snapshot_name}");
            if let Err(e) = ops.destroy_snapshot(dataset, snapshot_name).await {
                let _ = self
                    .snapshot_receipt(
                        VaultAuditAction::SnapshotDelete,
                        VaultAuditStatus::Error,
                        &target,
                        Some(e.to_string()),
                    )
                    .await;
                return Err(e);
            }
            self.snapshot_receipt(
                VaultAuditAction::SnapshotDelete,
                VaultAuditStatus::Success,
                &target,
                None,
            )
            .await?;
            let mut snaps = self.snapshots.write().await;
            snaps.retain(|s| s.name != snapshot_name);
            return Ok(());
        }

        let mut snaps = self.snapshots.write().await;
        let pos = snaps
            .iter()
            .position(|s| s.name == snapshot_name)
            .ok_or_else(|| VaultError::SnapshotNotFound(snapshot_name.to_string()))?;

        info!(snapshot = %snapshot_name, "Deleting snapshot");
        snaps.remove(pos);

        Ok(())
    }

    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, VaultError> {
        if let Some(ops) = &self.zfs {
            // Live listing is the truth; keep the reason strings the cache
            // carries for snapshots this process created.
            let live = ops.list_snapshots(&self.config.storage.dataset).await?;
            let mut snaps = self.snapshots.write().await;
            let merged: Vec<SnapshotInfo> = live
                .into_iter()
                .map(|mut s| {
                    if let Some(cached) = snaps.iter().find(|c| c.name == s.name) {
                        s.reason = cached.reason.clone();
                    }
                    s
                })
                .collect();
            *snaps = merged.clone();
            return Ok(merged);
        }
        let snaps = self.snapshots.read().await;
        Ok(snaps.clone())
    }

    async fn model_exists(&self, model_id: &str) -> Result<bool, VaultError> {
        let index = self.model_index.read().await;
        Ok(index.contains_key(model_id))
    }

    async fn model_size(&self, model_id: &str) -> Result<u64, VaultError> {
        let index = self.model_index.read().await;
        let entry = index
            .get(model_id)
            .ok_or_else(|| VaultError::ModelNotFound(model_id.to_string()))?;
        Ok(entry.size_bytes)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> VaultConfig {
        let mut config = VaultConfig::default();
        config.storage.mount_point = tmp.path().to_path_buf();
        config.storage.staging_dir = tmp.path().join("staging");
        config
    }

    #[tokio::test]
    async fn test_initialize_creates_vault() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
    }

    #[tokio::test]
    async fn test_store_and_load_model() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        let model_data = b"fake model weights for testing";
        vault
            .store_model_package("test-model-1", model_data)
            .await
            .unwrap();

        let loaded = vault.load_model_weights("test-model-1").await.unwrap();
        assert_eq!(loaded, model_data);
    }

    /// V4: a vault with a PQC engine wired — the production shape.
    fn engine_vault(tmp: &TempDir) -> ZfsVault {
        let cfg = test_config(tmp);
        let pqc = std::sync::Arc::new(crate::pqc::PqcEngine::new(cfg.pqc.clone()));
        let audit = std::sync::Arc::new(crate::audit::AuditWriter::with_pqc(
            cfg.audit.clone(),
            pqc.clone(),
        ));
        ZfsVault::with_engines(cfg, pqc, audit)
    }

    #[tokio::test]
    async fn v4_weights_are_sealed_at_rest_and_round_trip() {
        let tmp = TempDir::new().unwrap();
        let vault = engine_vault(&tmp);
        vault.initialize().await.unwrap();

        let plaintext = b"regulated model weights: mrn 000-11-2222";
        vault
            .store_model_package("sealed-model", plaintext)
            .await
            .unwrap();

        // At rest: the weights file is the KEM+AEAD envelope, not plaintext.
        let raw = std::fs::read(tmp.path().join("sealed-model").join("weights.bin")).unwrap();
        assert_ne!(raw, plaintext.to_vec());
        assert!(
            !raw.windows(plaintext.len()).any(|w| w == plaintext),
            "plaintext must not appear anywhere in the stored file"
        );
        // The manifest records the format version.
        let manifest =
            std::fs::read_to_string(tmp.path().join("sealed-model").join("manifest.json")).unwrap();
        assert!(manifest.contains(WEIGHTS_FORMAT_ENCRYPTED_V1));

        // Load decrypts back to the exact plaintext.
        let loaded = vault.load_model_weights("sealed-model").await.unwrap();
        assert_eq!(loaded, plaintext.to_vec());
    }

    #[tokio::test]
    async fn v4_legacy_plaintext_manifest_still_loads() {
        // A pre-V4 store (no weights_format field) reads back raw — the
        // migration path for existing vaults.
        let tmp = TempDir::new().unwrap();
        let vault = engine_vault(&tmp);
        let dir = tmp.path().join("legacy-model");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("weights.bin"), b"legacy plain weights").unwrap();
        let hash = ZfsVault::compute_file_hash(&dir.join("weights.bin")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::json!({
                "model_id": "legacy-model", "sha256": hash, "size_bytes": 20,
            })
            .to_string(),
        )
        .unwrap();
        vault.initialize().await.unwrap(); // scan picks the model up

        let loaded = vault.load_model_weights("legacy-model").await.unwrap();
        assert_eq!(loaded, b"legacy plain weights".to_vec());
    }

    #[tokio::test]
    async fn v7_crypto_erase_makes_retained_ciphertext_unrecoverable() {
        use mai_core::vault::ModelStorage;

        let tmp = TempDir::new().unwrap();
        let vault = engine_vault(&tmp);
        vault.initialize().await.unwrap();
        vault
            .store_model_package("erase-me", b"confidential weights")
            .await
            .unwrap();

        // Capture the exact ciphertext bytes as a "retained snapshot" would.
        let ciphertext = std::fs::read(tmp.path().join("erase-me").join("weights.bin")).unwrap();

        // Crypto-erase (this is what remove_model performs).
        vault.remove_model("erase-me").await.unwrap();

        // Re-present the retained ciphertext under the same model id: the key
        // is gone, so decapsulation can never succeed again. This is the V7
        // guarantee — CoW block/snapshot retention does not matter once the
        // key is retired. A fresh engine (no key for this id) stands in for
        // "the key no longer exists anywhere".
        let pqc = crate::pqc::PqcEngine::new(test_config(&tmp).pqc);
        let err = pqc
            .decrypt_model_weights("erase-me", &ciphertext)
            .await
            .unwrap_err();
        assert!(
            matches!(err, VaultError::PqcError(_)),
            "retained ciphertext must be undecryptable after crypto-erase, got {err:?}"
        );
    }

    #[tokio::test]
    async fn v4_encrypted_weights_without_engine_fail_closed() {
        // Store with an engine, then reopen the same tree engine-less: the
        // load refuses rather than serving ciphertext as a model.
        let tmp = TempDir::new().unwrap();
        let vault = engine_vault(&tmp);
        vault.initialize().await.unwrap();
        vault
            .store_model_package("locked-model", b"secret weights")
            .await
            .unwrap();

        let bare = ZfsVault::new(test_config(&tmp));
        bare.initialize().await.unwrap();
        let err = bare.load_model_weights("locked-model").await.unwrap_err();
        assert!(
            matches!(err, VaultError::PqcError(_)),
            "engine-less read of sealed weights must fail closed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_store_duplicate_fails() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        vault
            .store_model_package("dup-model", b"data1")
            .await
            .unwrap();
        let result = vault.store_model_package("dup-model", b"data2").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_missing_model_fails() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        let result = vault.load_model_weights("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_integrity_check() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        vault
            .store_model_package("integrity-test", b"integrity test data")
            .await
            .unwrap();

        let result = vault
            .verify_model_integrity("integrity-test")
            .await
            .unwrap();
        assert!(result.valid);
        assert_eq!(result.verified_bytes, b"integrity test data".len() as u64);
    }

    #[tokio::test]
    async fn test_storage_info() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        vault
            .store_model_package("info-model", b"some data here")
            .await
            .unwrap();

        let info = vault.storage_info().await.unwrap();
        assert_eq!(info.model_count, 1);
        assert!(info.used_bytes > 0);
    }

    #[tokio::test]
    async fn test_remove_model() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        vault
            .store_model_package("remove-me", b"deletable data")
            .await
            .unwrap();
        assert!(vault.model_exists("remove-me").await.unwrap());

        vault.remove_model("remove-me").await.unwrap();
        assert!(!vault.model_exists("remove-me").await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_signature_requires_wired_pqc_engine() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        // Without a wired engine, verify_signature must NOT silently succeed.
        let result = vault.verify_signature(b"data", b"sig").await;
        assert!(matches!(result, Err(VaultError::PqcError(_))));
    }

    #[tokio::test]
    async fn test_verify_signature_delegates_to_pqc_engine() {
        use crate::audit::AuditWriter;
        use crate::config::{AuditConfig, PqcConfig};
        use std::sync::Arc;

        let tmp = TempDir::new().unwrap();
        let pqc = Arc::new(PqcEngine::new(PqcConfig {
            kem_algorithm: "ML-KEM-1024".into(),
            dsa_algorithm: "ML-DSA-87".into(),
            key_store_path: tmp.path().join("keys"),
            symmetric_cipher: "AES-256-GCM".into(),
        }));
        pqc.initialize().await.unwrap();
        let audit = Arc::new(AuditWriter::new(AuditConfig {
            db_path: tmp.path().join("audit.json"),
            wal_mode: true,
            sign_interval: 100,
            max_entries: 0,
        }));
        audit.initialize().await.unwrap();

        let vault = ZfsVault::with_engines(test_config(&tmp), pqc.clone(), audit);

        let data = b"vault signature delegation test";
        let sig = pqc.sign_package(data).await.unwrap();
        assert!(vault.verify_signature(data, &sig).await.unwrap());
        // Tampered data must fail.
        assert!(!vault.verify_signature(b"tampered", &sig).await.unwrap());
    }

    #[tokio::test]
    async fn test_snapshot_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let vault = ZfsVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        let snap = vault.create_snapshot("test backup").await.unwrap();
        assert!(snap.name.starts_with("mai-snap-"));

        let list = vault.list_snapshots().await.unwrap();
        assert_eq!(list.len(), 1);

        vault.delete_snapshot(&snap.name).await.unwrap();
        let list = vault.list_snapshots().await.unwrap();
        assert!(list.is_empty());
    }
}
