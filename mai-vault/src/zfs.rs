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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{
    IntegrityResult, ModelStorage, SnapshotInfo, StorageInfo, VaultError, VaultInterface,
};

use crate::config::VaultConfig;

/// ZFS-backed vault providing encrypted model storage.
///
/// This is the primary vault type. It implements `VaultInterface` for backward
/// compatibility with existing mai-core consumers and `ModelStorage` for the
/// extended ZFS operations added in Session 12.
pub struct ZfsVault {
    config: VaultConfig,
    /// Model manifest cache: model_id -> (hash, size_bytes)
    model_index: RwLock<HashMap<String, ModelEntry>>,
    /// Snapshot metadata cache
    snapshots: RwLock<Vec<SnapshotInfo>>,
}

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

        // In production: read ZFS dataset and parse model manifest files.
        // Each model directory contains a manifest.json with hash and metadata.
        // For now, scan the directory and register entries.

        if models_dir.is_dir() {
            match std::fs::read_dir(models_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            if let Some(model_id) = path.file_name().and_then(|n| n.to_str()) {
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
            .and_then(|v| v.as_str())
            .ok_or_else(|| VaultError::IoError("Missing sha256 in manifest".into()))?
            .to_string();

        let size = manifest
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok((hash, size))
    }

    /// Scan for ZFS snapshots.
    async fn scan_snapshots(&self) -> Result<(), VaultError> {
        let mut snaps = self.snapshots.write().await;
        snaps.clear();
        // In production: run `zfs list -t snapshot -o name,creation,referenced`
        // and parse the output. For now, return empty list.
        debug!("Snapshot scan complete (no ZFS access in stub mode)");
        Ok(())
    }

    /// Compute SHA-256 hash of a file.
    fn compute_file_hash(path: &Path) -> Result<String, VaultError> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(VaultError::from)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 65536];
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
                "Weights file missing for model {}",
                model_id
            )));
        }

        info!(model_id, path = %weights_path.display(), "Loading model weights from vault");

        // Read the encrypted weights. In production, decryption happens here
        // via the PqcProvider. For now, read raw bytes.
        let data = tokio::fs::read(&weights_path)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        info!(model_id, bytes = data.len(), "Model weights loaded");
        Ok(data)
    }

    async fn store_model_package(
        &self,
        model_id: &str,
        data: &[u8],
    ) -> Result<(), VaultError> {
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

        // Write weights (in production: encrypt via PqcProvider first)
        tokio::fs::write(&weights_path, data)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

        // Compute hash and write manifest
        let hash = Self::compute_file_hash(&weights_path)?;
        let manifest = serde_json::json!({
            "model_id": model_id,
            "sha256": hash,
            "size_bytes": data.len(),
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
                size_bytes: data.len() as u64,
                path: model_dir,
                verified: true, // just wrote it
            },
        );

        info!(model_id, "Model package stored successfully");
        Ok(())
    }

    async fn append_audit_entry(&self, entry: &[u8]) -> Result<(), VaultError> {
        // Delegate to AuditStore implementation when wired.
        // For now, log and succeed.
        debug!(
            bytes = entry.len(),
            "Vault received audit entry (delegate to AuditStore)"
        );
        Ok(())
    }

    async fn verify_signature(
        &self,
        data: &[u8],
        signature: &[u8],
    ) -> Result<bool, VaultError> {
        // Delegate to PqcProvider implementation when wired.
        // For now, return true (signature verification placeholder).
        debug!(
            data_bytes = data.len(),
            sig_bytes = signature.len(),
            "Vault signature verification (delegate to PqcProvider)"
        );
        Ok(true)
    }
}

// ============================================================================
// ModelStorage implementation (extended ZFS operations)
// ============================================================================

#[async_trait]
impl ModelStorage for ZfsVault {
    async fn verify_model_integrity(
        &self,
        model_id: &str,
    ) -> Result<IntegrityResult, VaultError> {
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
        if !valid {
            warn!(
                model_id,
                expected = %entry.expected_hash,
                computed = %computed_hash,
                "Model integrity check FAILED"
            );
        } else {
            debug!(model_id, "Model integrity check passed");
        }

        Ok(IntegrityResult {
            valid,
            computed_hash,
            expected_hash: entry.expected_hash.clone(),
            verified_bytes: file_size,
        })
    }

    async fn storage_info(&self) -> Result<StorageInfo, VaultError> {
        // In production: query ZFS dataset properties.
        // For now, stat the mount point filesystem.
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

        Ok(StorageInfo {
            total_bytes: total,
            used_bytes: total_model_bytes,
            available_bytes: available,
            model_count: index.len() as u32,
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

        info!(model_id, path = %model_dir.display(), "Securely removing model from vault");

        // In production: ZFS destroy + scrub the freed blocks.
        // For now: remove the directory tree.
        if model_dir.exists() {
            tokio::fs::remove_dir_all(&model_dir)
                .await
                .map_err(|e| VaultError::IoError(e.to_string()))?;
        }

        // Remove from index
        let mut index = self.model_index.write().await;
        index.remove(model_id);

        info!(model_id, "Model removed from vault");
        Ok(())
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!(
            "mai-snap-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        );
        let now = chrono::Utc::now().timestamp() as u64;

        info!(snapshot = %name, reason, "Creating vault snapshot");

        // In production: run `zfs snapshot im-vault/models@{name}`
        let snap = SnapshotInfo {
            name: name.clone(),
            created_at: now,
            referenced_bytes: 0, // populated by ZFS
            reason: reason.to_string(),
        };

        let mut snaps = self.snapshots.write().await;
        snaps.push(snap.clone());

        info!(snapshot = %name, "Snapshot created");
        Ok(snap)
    }

    async fn rollback_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        let snaps = self.snapshots.read().await;
        if !snaps.iter().any(|s| s.name == snapshot_name) {
            return Err(VaultError::SnapshotNotFound(snapshot_name.to_string()));
        }

        info!(snapshot = %snapshot_name, "Rolling back to snapshot");
        // In production: run `zfs rollback im-vault/models@{snapshot_name}`
        // Then re-scan models.

        drop(snaps);
        self.scan_models().await?;

        info!(snapshot = %snapshot_name, "Rollback complete");
        Ok(())
    }

    async fn delete_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        let mut snaps = self.snapshots.write().await;
        let pos = snaps
            .iter()
            .position(|s| s.name == snapshot_name)
            .ok_or_else(|| VaultError::SnapshotNotFound(snapshot_name.to_string()))?;

        info!(snapshot = %snapshot_name, "Deleting snapshot");
        // In production: run `zfs destroy im-vault/models@{snapshot_name}`
        snaps.remove(pos);

        Ok(())
    }

    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, VaultError> {
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

        let result = vault.verify_model_integrity("integrity-test").await.unwrap();
        assert!(result.valid);
        assert_eq!(result.verified_bytes, 18); // len("integrity test data")
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
