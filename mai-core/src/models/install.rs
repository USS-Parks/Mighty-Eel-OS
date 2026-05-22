use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::registry::{InstallResult, ModelRegistry, RegistryError};
use crate::vault::{ModelStorage, VaultInterface};

use super::package::ModelPackage;
use super::verify;

/// Current state of an installation operation
#[derive(Debug, Clone)]
pub enum InstallProgress {
    /// Package discovered, reading manifest
    Discovering,
    /// Verifying package integrity (PQC + hash tree)
    Verifying { step: u8, total: u8 },
    /// Creating ZFS snapshot
    Snapshotting,
    /// Storing weights in vault
    Storing { percent: u8 },
    /// Registering model in cold storage
    Registering,
    /// Writing audit trail
    Auditing,
    /// Installation complete
    Completed { model_id: String, elapsed_secs: f64 },
    /// Installation failed
    Failed { error: String },
}

impl InstallProgress {
    /// Human-readable status string
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::Discovering => "discovering",
            Self::Verifying { .. } => "verifying",
            Self::Snapshotting => "snapshotting",
            Self::Storing { .. } => "storing",
            Self::Registering => "registering",
            Self::Auditing => "auditing",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
        }
    }

    /// Progress percentage (0-100)
    pub fn percent(&self) -> u8 {
        match self {
            Self::Discovering => 5,
            Self::Verifying { step, total } if *total > 0 => 10 + ((step * 70) / total),
            Self::Verifying { .. } => 10,
            Self::Snapshotting => 50,
            Self::Storing { percent } => 50 + (percent / 2),
            Self::Registering => 85,
            Self::Auditing => 95,
            Self::Completed { .. } => 100,
            Self::Failed { .. } => 0,
        }
    }
}

/// Install a model package into the registry with full verification
///
/// Returns the model ID on success. Progress is reported through the
/// optional `progress` callback.
pub async fn install_package<V, S>(
    pkg: &ModelPackage,
    registry: &Arc<RwLock<ModelRegistry>>,
    vault: &V,
    storage: &S,
    current_mai_version: &str,
    progress: Option<&dyn Fn(InstallProgress)>,
) -> Result<InstallResult, RegistryError>
where
    V: VaultInterface,
    S: ModelStorage,
{
    let start = Instant::now();
    let _ = progress.map(|p| p(InstallProgress::Discovering));

    // 1. Create ZFS snapshot before installation
    let _ = progress.map(|p| p(InstallProgress::Snapshotting));
    if let Err(e) = storage.create_snapshot("pre-install").await {
        warn!(error = %e, "Failed to create pre-install snapshot, continuing");
    }

    // 2. Verify package
    let _ = progress.map(|p| p(InstallProgress::Verifying { step: 0, total: 3 }));
    let verify_result = verify::verify_package(pkg, vault, current_mai_version).await;
    if !verify_result.verified {
        return Err(RegistryError::SignatureVerificationFailed(
            verify_result
                .messages
                .first()
                .cloned()
                .unwrap_or_else(|| "Package verification failed".to_string()),
        ));
    }
    let _ = progress.map(|p| p(InstallProgress::Verifying { step: 1, total: 3 }));

    // 3. Read weights
    let weights = pkg
        .read_weights()
        .map_err(|e| RegistryError::UsbPackageError(format!("Failed to read weights: {e}")))?;
    let _ = progress.map(|p| p(InstallProgress::Verifying { step: 2, total: 3 }));

    // 4. Store in vault
    let model_id = pkg.model_id();
    let _ = progress.map(|p| p(InstallProgress::Storing { percent: 10 }));
    vault.store_model_package(&model_id, &weights).await?;
    let _ = progress.map(|p| p(InstallProgress::Storing { percent: 50 }));

    // 5. Create vault path and register in cold storage
    let vault_path = PathBuf::from(format!("/vault/models/{model_id}"));
    let _ = progress.map(|p| p(InstallProgress::Registering));

    let mut reg = registry.write().await;
    reg.register_cold_model(model_id.clone(), pkg.manifest.clone(), vault_path)
        .await?;
    drop(reg);

    let _ = progress.map(|p| p(InstallProgress::Auditing));

    // 6. Write audit entry
    let audit_entry = format!(
        "{{\"event\":\"model_installed\",\"model_id\":\"{model_id}\",\"source\":\"usb\",\"package\":\"{}\"}}",
        pkg.name
    );
    if let Err(e) = vault.append_audit_entry(audit_entry.as_bytes()).await {
        warn!(error = %e, "Failed to write audit entry for USB install");
    }

    let elapsed = start.elapsed().as_secs_f64();
    info!(
        model_id = %model_id,
        elapsed_secs = elapsed,
        "Model installed successfully"
    );

    let result = InstallResult {
        model_id,
        installed_at: Instant::now(),
        integrity_verified: verify_result.hash_tree_valid,
        signature_verified: verify_result.signature_valid,
    };

    let _ = progress.map(|p| {
        p(InstallProgress::Completed {
            model_id: result.model_id.clone(),
            elapsed_secs: elapsed,
        })
    });

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModelRegistry;
    use crate::vault::{SnapshotInfo, VaultError};
    use std::path::Path;
    use std::sync::Arc;
    use tokio::sync::RwLock as TokioRwLock;

    struct MockVault;

    #[async_trait::async_trait]
    impl VaultInterface for MockVault {
        async fn load_model_weights(&self, _model_id: &str) -> Result<Vec<u8>, VaultError> {
            Ok(vec![0u8; 1024])
        }
        async fn store_model_package(
            &self,
            _model_id: &str,
            _data: &[u8],
        ) -> Result<(), VaultError> {
            Ok(())
        }
        async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
            Ok(())
        }
        async fn verify_signature(
            &self,
            _data: &[u8],
            _signature: &[u8],
        ) -> Result<bool, VaultError> {
            Ok(true)
        }
    }

    #[async_trait::async_trait]
    impl ModelStorage for MockVault {
        async fn verify_model_integrity(
            &self,
            _model_id: &str,
        ) -> Result<crate::vault::IntegrityResult, VaultError> {
            Ok(crate::vault::IntegrityResult {
                valid: true,
                expected_hash: "root".to_string(),
                computed_hash: "root".to_string(),
                verified_bytes: 100,
            })
        }
        async fn storage_info(&self) -> Result<crate::vault::StorageInfo, VaultError> {
            Ok(crate::vault::StorageInfo {
                total_bytes: 1_000_000_000_000,
                used_bytes: 0,
                available_bytes: 1_000_000_000_000,
                model_count: 0,
                compression_ratio: 1.0,
            })
        }
        async fn remove_model(&self, _model_id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        async fn create_snapshot(&self, _reason: &str) -> Result<SnapshotInfo, VaultError> {
            Ok(SnapshotInfo {
                name: "test-snap".into(),
                created_at: 0,
                referenced_bytes: 0,
                reason: "test".into(),
            })
        }
        async fn rollback_snapshot(&self, _snapshot_name: &str) -> Result<(), VaultError> {
            Ok(())
        }
        async fn delete_snapshot(&self, _snapshot_name: &str) -> Result<(), VaultError> {
            Ok(())
        }
        async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, VaultError> {
            Ok(vec![])
        }
        async fn model_exists(&self, _model_id: &str) -> Result<bool, VaultError> {
            Ok(true)
        }
        async fn model_size(&self, _model_id: &str) -> Result<u64, VaultError> {
            Ok(1000)
        }
    }

    fn create_test_package(dir: &Path) -> ModelPackage {
        use crate::models::verify::compute_hash_tree_root;
        use std::fs;
        let pkg_dir = dir.join("test-model.mai-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest = r#"
[model]
name = "test-model"
version = "1.0.0"
format = "GGUF"
quantization = "Q4_K_M"
size_bytes = 1000
required_vram_bytes = 2000

[compatibility]
min_mai_version = "0.1.0"
supported_backends = ["ollama"]
hardware_classes = ["cpu"]

[capabilities]
chat = true
completion = true
embedding = false
vision = false
structured_output = false
max_context_tokens = 4096
supported_languages = ["en"]

[security]
signature_algorithm = "ML-DSA-87"
public_key_fingerprint = "sha256:test"
integrity_hash_tree = "root_hash"

[metadata]
license = "MIT"
changelog = "Initial"
"#;
        fs::write(pkg_dir.join("manifest.toml"), manifest).unwrap();

        let weights = vec![0u8; 100];
        let hash_root = compute_hash_tree_root(&weights);
        fs::write(pkg_dir.join("weights.bin"), &weights).unwrap();
        fs::write(pkg_dir.join("signature.mldsa"), vec![1u8; 64]).unwrap();
        fs::write(pkg_dir.join("hash_tree.sha256"), format!("{hash_root}\n")).unwrap();

        ModelPackage::open(&pkg_dir).unwrap()
    }

    #[tokio::test]
    async fn test_install_package_success() {
        let dir = std::env::temp_dir().join("test_install_success");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let pkg = create_test_package(&dir);
        let vault = MockVault;
        let registry = Arc::new(TokioRwLock::new(ModelRegistry::new(Box::new(MockVault))));

        let result = install_package(&pkg, &registry, &vault, &vault, "0.2.0", None).await;

        assert!(result.is_ok());
        let install_result = result.unwrap();
        assert_eq!(install_result.model_id, "test-model:1.0.0:Q4_K_M");
        assert!(install_result.signature_verified);
        assert!(install_result.integrity_verified);

        let reg = registry.read().await;
        let model_id = "test-model:1.0.0:Q4_K_M".to_string();
        assert!(reg.get_model(&model_id).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_install_package_failed_verification() {
        let dir = std::env::temp_dir().join("test_install_fail_verify");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let pkg = create_test_package(&dir);
        struct BadVault;
        #[async_trait::async_trait]
        impl VaultInterface for BadVault {
            async fn load_model_weights(&self, _id: &str) -> Result<Vec<u8>, VaultError> {
                Ok(vec![])
            }
            async fn store_model_package(&self, _id: &str, _data: &[u8]) -> Result<(), VaultError> {
                Ok(())
            }
            async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
                Ok(())
            }
            async fn verify_signature(
                &self,
                _data: &[u8],
                _sig: &[u8],
            ) -> Result<bool, VaultError> {
                Ok(false)
            }
        }
        #[async_trait::async_trait]
        impl ModelStorage for BadVault {
            async fn verify_model_integrity(
                &self,
                _id: &str,
            ) -> Result<crate::vault::IntegrityResult, VaultError> {
                Ok(crate::vault::IntegrityResult {
                    valid: true,
                    expected_hash: "".into(),
                    computed_hash: "".into(),
                    verified_bytes: 0,
                })
            }
            async fn storage_info(&self) -> Result<crate::vault::StorageInfo, VaultError> {
                Ok(crate::vault::StorageInfo {
                    total_bytes: 0,
                    used_bytes: 0,
                    available_bytes: 0,
                    model_count: 0,
                    compression_ratio: 1.0,
                })
            }
            async fn remove_model(&self, _id: &str) -> Result<(), VaultError> {
                Ok(())
            }
            async fn create_snapshot(&self, _reason: &str) -> Result<SnapshotInfo, VaultError> {
                Ok(SnapshotInfo {
                    name: "snap".into(),
                    created_at: 0,
                    referenced_bytes: 0,
                    reason: "test".into(),
                })
            }
            async fn rollback_snapshot(&self, _name: &str) -> Result<(), VaultError> {
                Ok(())
            }
            async fn delete_snapshot(&self, _name: &str) -> Result<(), VaultError> {
                Ok(())
            }
            async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, VaultError> {
                Ok(vec![])
            }
            async fn model_exists(&self, _id: &str) -> Result<bool, VaultError> {
                Ok(false)
            }
            async fn model_size(&self, _id: &str) -> Result<u64, VaultError> {
                Ok(0)
            }
        }

        let registry = Arc::new(TokioRwLock::new(ModelRegistry::new(Box::new(BadVault))));
        let result = install_package(&pkg, &registry, &BadVault, &BadVault, "0.2.0", None).await;
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_install_progress_states() {
        let states: Vec<InstallProgress> = vec![
            InstallProgress::Discovering,
            InstallProgress::Verifying { step: 0, total: 3 },
            InstallProgress::Verifying { step: 2, total: 3 },
            InstallProgress::Snapshotting,
            InstallProgress::Storing { percent: 10 },
            InstallProgress::Storing { percent: 100 },
            InstallProgress::Registering,
            InstallProgress::Auditing,
            InstallProgress::Completed {
                model_id: "test".into(),
                elapsed_secs: 1.5,
            },
            InstallProgress::Failed {
                error: "test error".into(),
            },
        ];

        for state in &states {
            let _ = state.status_str();
            let _ = state.percent();
        }

        assert_eq!(states[0].status_str(), "discovering");
        assert_eq!(states[8].status_str(), "completed");
        assert_eq!(states[9].status_str(), "failed");
        assert_eq!(states[8].percent(), 100);
        assert_eq!(states[9].percent(), 0);
    }
}
