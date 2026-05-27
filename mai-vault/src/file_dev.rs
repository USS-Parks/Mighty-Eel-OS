//! FileDev vault — filesystem-backed model storage for staging/dev/Windows.
//!
//! Production-capable vault backend that uses the local filesystem instead
//! of ZFS. Identical to ZfsVault in public API, different in deployment
//! contract: no ZFS dataset required, no snapshot-on-ZFS semantics.
//!
//! # Architecture
//!
//! ```text
//! FileDevVault
//!   ├── VaultInterface   (load / store / append_audit / verify_signature)
//!   ├── ModelStorage      (integrity / storage_info / snapshots / remove)
//!   └── Plain filesystem
//!       ├── <root>/<model_id>/weights.bin
//!       ├── <root>/<model_id>/manifest.json
//!       └── <root>/snapshots/  (metadata-only)
//! ```
//!
//! This replaces ZFS datasets with plain directories. Snapshots are metadata-
//! only (no COW); integrity is file-hash-based (BLAKE3).

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use mai_core::vault::{
    AuditStore, IntegrityResult, ModelStorage, PqcProvider, SnapshotInfo, StorageInfo, VaultError,
    VaultInterface,
};

use crate::audit::AuditWriter;
use crate::config::VaultConfig;
use crate::pqc::PqcEngine;

/// Filesystem-backed vault for staging, dev, and Windows deployments.
///
/// Same public API as [`crate::zfs::ZfsVault`], but the storage layer is
/// plain directories on the host filesystem. Suitable for production mode
/// where ZFS is not available (Windows, macOS, CI, staging appliances).
pub struct FileDevVault {
    config: VaultConfig,
    model_index: RwLock<HashMap<String, ModelEntry>>,
    snapshots: RwLock<Vec<SnapshotInfo>>,
    pqc: Option<Arc<PqcEngine>>,
    audit: Option<Arc<AuditWriter>>,
}

struct ModelEntry {
    expected_hash: String,
    size_bytes: u64,
    path: PathBuf,
    verified: bool,
}

impl FileDevVault {
    pub fn new(config: VaultConfig) -> Self {
        Self {
            config,
            model_index: RwLock::new(HashMap::new()),
            snapshots: RwLock::new(Vec::new()),
            pqc: None,
            audit: None,
        }
    }

    pub fn with_engines(config: VaultConfig, pqc: Arc<PqcEngine>, audit: Arc<AuditWriter>) -> Self {
        Self {
            config,
            model_index: RwLock::new(HashMap::new()),
            snapshots: RwLock::new(Vec::new()),
            pqc: Some(pqc),
            audit: Some(audit),
        }
    }

    pub async fn initialize(&self) -> Result<(), VaultError> {
        info!(
            root = %self.config.storage.mount_point.display(),
            "Initializing FileDev vault"
        );

        let mount = &self.config.storage.mount_point;
        if !mount.exists() {
            std::fs::create_dir_all(mount)
                .map_err(|e| VaultError::IoError(format!("create vault root: {e}")))?;
        }

        // Ensure snapshots subdir exists
        let snap_dir = mount.join("snapshots");
        std::fs::create_dir_all(&snap_dir).ok();

        self.scan_models().await?;
        self.scan_snapshots().await?;

        info!("FileDev vault initialized successfully");
        Ok(())
    }

    async fn scan_models(&self) -> Result<(), VaultError> {
        let models_dir = &self.config.storage.mount_point;
        let mut index = self.model_index.write().await;
        index.clear();

        if models_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(models_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir()
                        && path.file_name().and_then(|n| n.to_str()) != Some("snapshots")
                        && let Some(model_id) = path.file_name().and_then(|n| n.to_str())
                    {
                        let manifest_path = path.join("manifest.json");
                        if manifest_path.exists() {
                            match Self::read_model_manifest(&manifest_path) {
                                Ok((hash, size)) => {
                                    debug!(model_id, hash = %hash, size, "Found model");
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

        info!(model_count = index.len(), "Model scan complete");
        Ok(())
    }

    async fn scan_snapshots(&self) -> Result<(), VaultError> {
        // Read snapshot manifests from <root>/snapshots/
        let snap_dir = self.config.storage.mount_point.join("snapshots");
        let mut snaps = self.snapshots.write().await;
        snaps.clear();

        if let Ok(entries) = std::fs::read_dir(&snap_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(snap) = serde_json::from_str::<SnapshotInfo>(&content) {
                            snaps.push(snap);
                        }
                    }
                }
            }
        }
        Ok(())
    }

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
}

// ─── VaultInterface ─────────────────────────────────────────────────

#[async_trait]
impl VaultInterface for FileDevVault {
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
        info!(model_id, path = %weights_path.display(), "Loading model weights");
        tokio::fs::read(&weights_path)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))
    }

    async fn store_model_package(&self, model_id: &str, data: &[u8]) -> Result<(), VaultError> {
        {
            let index = self.model_index.read().await;
            if index.contains_key(model_id) {
                return Err(VaultError::ModelAlreadyExists(model_id.to_string()));
            }
        }
        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");

        info!(model_id, bytes = data.len(), path = %model_dir.display(), "Storing model");

        tokio::fs::create_dir_all(&model_dir)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;
        tokio::fs::write(&weights_path, data)
            .await
            .map_err(|e| VaultError::IoError(e.to_string()))?;

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

        let mut index = self.model_index.write().await;
        index.insert(
            model_id.to_string(),
            ModelEntry {
                expected_hash: hash,
                size_bytes: data.len() as u64,
                path: model_dir,
                verified: true,
            },
        );
        info!(model_id, "Model package stored");
        Ok(())
    }

    async fn append_audit_entry(&self, entry: &[u8]) -> Result<(), VaultError> {
        let audit = self
            .audit
            .as_ref()
            .ok_or_else(|| VaultError::AuditStoreError("AuditWriter not wired".into()))?;
        let parsed: mai_core::vault::VaultAuditEntry = serde_json::from_slice(entry)
            .map_err(|e| VaultError::AuditStoreError(format!("audit entry decode: {e}")))?;
        debug!(entry_id = %parsed.entry_id, "Delegating audit entry to AuditWriter");
        audit.append(&parsed).await
    }

    async fn verify_signature(&self, data: &[u8], signature: &[u8]) -> Result<bool, VaultError> {
        let pqc = self
            .pqc
            .as_ref()
            .ok_or_else(|| VaultError::PqcError("PqcEngine not wired".into()))?;
        pqc.verify_package(data, signature).await
    }
}

// ─── ModelStorage ───────────────────────────────────────────────────

#[async_trait]
impl ModelStorage for FileDevVault {
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
        if !valid {
            warn!(model_id, expected = %entry.expected_hash, computed = %computed_hash, "Integrity FAILED");
        }
        Ok(IntegrityResult {
            valid,
            computed_hash,
            expected_hash: entry.expected_hash.clone(),
            verified_bytes: file_size,
        })
    }

    async fn storage_info(&self) -> Result<StorageInfo, VaultError> {
        let index = self.model_index.read().await;
        let total_model_bytes: u64 = index.values().map(|e| e.size_bytes).sum();
        let total = if self.config.storage.max_capacity_bytes > 0 {
            self.config.storage.max_capacity_bytes
        } else {
            1_099_511_627_776
        };
        let available = total.saturating_sub(total_model_bytes);
        Ok(StorageInfo {
            total_bytes: total,
            used_bytes: total_model_bytes,
            available_bytes: available,
            model_count: index.len() as u32,
            compression_ratio: 1.0,
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
        info!(model_id, path = %model_dir.display(), "Removing model");
        if model_dir.exists() {
            tokio::fs::remove_dir_all(&model_dir)
                .await
                .map_err(|e| VaultError::IoError(e.to_string()))?;
        }
        let mut index = self.model_index.write().await;
        index.remove(model_id);
        Ok(())
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!("mai-snap-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let now = chrono::Utc::now().timestamp() as u64;
        info!(snapshot = %name, reason, "Creating snapshot");
        let snap = SnapshotInfo {
            name: name.clone(),
            created_at: now,
            referenced_bytes: 0,
            reason: reason.to_string(),
        };
        // Persist snapshot metadata
        let snap_path = self
            .config
            .storage
            .mount_point
            .join("snapshots")
            .join(format!("{name}.json"));
        if let Ok(json) = serde_json::to_string(&snap) {
            let _ = std::fs::write(&snap_path, json);
        }
        let mut snaps = self.snapshots.write().await;
        snaps.push(snap.clone());
        Ok(snap)
    }

    async fn rollback_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        let snaps = self.snapshots.read().await;
        if !snaps.iter().any(|s| s.name == snapshot_name) {
            return Err(VaultError::SnapshotNotFound(snapshot_name.to_string()));
        }
        drop(snaps);
        // Metadata-only: re-scan models to reflect post-rollback state
        self.scan_models().await
    }

    async fn delete_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError> {
        let mut snaps = self.snapshots.write().await;
        let pos = snaps
            .iter()
            .position(|s| s.name == snapshot_name)
            .ok_or_else(|| VaultError::SnapshotNotFound(snapshot_name.to_string()))?;
        snaps.remove(pos);
        // Remove persisted metadata
        let snap_path = self
            .config
            .storage
            .mount_point
            .join("snapshots")
            .join(format!("{snapshot_name}.json"));
        let _ = std::fs::remove_file(&snap_path);
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

// ─── Tests ──────────────────────────────────────────────────────────

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
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
    }

    #[tokio::test]
    async fn test_store_and_load_model() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();

        let model_data = b"fake model weights";
        vault
            .store_model_package("test-model", model_data)
            .await
            .unwrap();
        let loaded = vault.load_model_weights("test-model").await.unwrap();
        assert_eq!(loaded, model_data);
    }

    #[tokio::test]
    async fn test_store_duplicate_fails() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        vault.store_model_package("dup", b"data1").await.unwrap();
        assert!(vault.store_model_package("dup", b"data2").await.is_err());
    }

    #[tokio::test]
    async fn test_load_missing_fails() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        assert!(vault.load_model_weights("nope").await.is_err());
    }

    #[tokio::test]
    async fn test_integrity_check() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        vault
            .store_model_package("int-test", b"integrity data")
            .await
            .unwrap();
        let result = vault.verify_model_integrity("int-test").await.unwrap();
        assert!(result.valid);
    }

    #[tokio::test]
    async fn test_remove_model() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        vault
            .store_model_package("remove-me", b"data")
            .await
            .unwrap();
        assert!(vault.model_exists("remove-me").await.unwrap());
        vault.remove_model("remove-me").await.unwrap();
        assert!(!vault.model_exists("remove-me").await.unwrap());
    }

    #[tokio::test]
    async fn test_snapshot_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        let snap = vault.create_snapshot("backup").await.unwrap();
        assert!(snap.name.starts_with("mai-snap-"));
        assert_eq!(vault.list_snapshots().await.unwrap().len(), 1);
        vault.delete_snapshot(&snap.name).await.unwrap();
        assert!(vault.list_snapshots().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_storage_info() {
        let tmp = TempDir::new().unwrap();
        let vault = FileDevVault::new(test_config(&tmp));
        vault.initialize().await.unwrap();
        vault
            .store_model_package("info-m", b"hello world")
            .await
            .unwrap();
        let info = vault.storage_info().await.unwrap();
        assert_eq!(info.model_count, 1);
        assert!(info.used_bytes > 0);
    }
}
