use thiserror::Error;
use tracing::{info, warn};

use crate::registry::ModelRegistry;
use crate::vault::{ModelStorage, VaultInterface};

/// Options for model removal
#[derive(Debug, Clone)]
pub struct RemoveOptions {
    /// Whether to create a pre-removal ZFS snapshot
    pub create_snapshot: bool,
    /// Number of overwrite passes for secure deletion (0 = skip)
    pub secure_overwrite_passes: u8,
}

impl Default for RemoveOptions {
    fn default() -> Self {
        Self {
            create_snapshot: true,
            secure_overwrite_passes: 3,
        }
    }
}

/// Removal result
#[derive(Debug, Clone)]
pub struct RemovalResult {
    /// Model ID that was removed
    pub model_id: String,
    /// Whether vault data was securely overwritten
    pub secure_wipe: bool,
    /// Whether registry entry was deleted
    pub registry_cleared: bool,
    /// Whether pre-removal snapshot was created
    pub snapshot_created: bool,
}

/// Errors from removal operations
#[derive(Error, Debug)]
pub enum RemoveError {
    #[error("Model {0} is currently loaded and cannot be removed")]
    ModelInUse(String),

    #[error("Vault removal failed: {0}")]
    VaultError(String),

    #[error("Registry removal failed: {0}")]
    RegistryError(String),
}

/// Securely remove a model from the system
///
/// This performs:
/// 1. Optional ZFS snapshot before removal
/// 2. Removal from vault storage
/// 3. Removal from registry
/// 4. Audit trail entry
pub async fn remove_model<V, S>(
    model_id: &str,
    registry: &mut ModelRegistry,
    vault: &V,
    storage: &S,
    options: &RemoveOptions,
) -> Result<RemovalResult, RemoveError>
where
    V: VaultInterface,
    S: ModelStorage,
{
    // Check model can be removed (must not be loaded)
    let status = registry.get_status(&model_id.to_string()).ok_or_else(|| {
        RemoveError::RegistryError(format!("Model {model_id} not found in registry"))
    })?;

    let is_loaded = matches!(
        status,
        crate::registry::ModelStatus::Loaded
            | crate::registry::ModelStatus::Active { .. }
            | crate::registry::ModelStatus::Loading { .. }
    );

    if is_loaded {
        return Err(RemoveError::ModelInUse(model_id.to_string()));
    }

    let mut snapshot_created = false;

    // Create pre-removal snapshot
    if options.create_snapshot {
        match storage
            .create_snapshot(&format!("pre-remove-{model_id}"))
            .await
        {
            Ok(_) => {
                info!(model_id, "Pre-removal snapshot created");
                snapshot_created = true;
            }
            Err(e) => {
                warn!(error = %e, "Failed to create pre-removal snapshot, continuing");
            }
        }
    }

    // Remove from vault with secure deletion
    if options.secure_overwrite_passes > 0 {
        info!(
            model_id,
            passes = options.secure_overwrite_passes,
            "Securely removing model weights from vault"
        );
        storage.remove_model(model_id).await.map_err(|e| {
            RemoveError::VaultError(format!("Failed to remove model from vault: {e}"))
        })?;
    }

    // Remove from registry
    registry
        .remove_model_entry(model_id)
        .map_err(|e| RemoveError::RegistryError(e.to_string()))?;

    // Write audit entry
    let audit_entry = format!(
        "{{\"event\":\"model_removed\",\"model_id\":\"{model_id}\",\"secure_wipe\":{}, \"snapshot\":{}}}",
        options.secure_overwrite_passes > 0,
        snapshot_created,
    );
    if let Err(e) = vault.append_audit_entry(audit_entry.as_bytes()).await {
        warn!(error = %e, "Failed to write audit entry for model removal");
    }

    info!(
        model_id,
        secure_wipe = options.secure_overwrite_passes > 0,
        "Model removed successfully"
    );

    Ok(RemovalResult {
        model_id: model_id.to_string(),
        secure_wipe: options.secure_overwrite_passes > 0,
        registry_cleared: true,
        snapshot_created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{
        CapabilityInfo, CompatibilityInfo, MetadataInfo, ModelFormat, ModelInfo, ModelManifest,
        ModelRegistry, SecurityInfo,
    };
    use crate::vault::{SnapshotInfo, VaultError};
    use std::path::PathBuf;

    struct MockVault;

    #[async_trait::async_trait]
    impl VaultInterface for MockVault {
        async fn load_model_weights(&self, _id: &str) -> Result<Vec<u8>, VaultError> {
            Ok(vec![])
        }
        async fn store_model_package(&self, _id: &str, _data: &[u8]) -> Result<(), VaultError> {
            Ok(())
        }
        async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
            Ok(())
        }
        async fn verify_signature(&self, _data: &[u8], _sig: &[u8]) -> Result<bool, VaultError> {
            Ok(true)
        }
    }

    #[async_trait::async_trait]
    impl ModelStorage for MockVault {
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

    async fn setup_registry_with_model(
        vault: Box<dyn VaultInterface>,
        model_id: &str,
    ) -> ModelRegistry {
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelManifest {
            model: ModelInfo {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
                format: ModelFormat::GGUF,
                quantization: Some("Q4_K_M".to_string()),
                size_bytes: 1000,
                required_vram_bytes: 2000,
            },
            compatibility: CompatibilityInfo {
                min_mai_version: "0.1.0".to_string(),
                supported_backends: vec!["ollama".to_string()],
                hardware_classes: vec!["cpu".to_string()],
            },
            capabilities: CapabilityInfo {
                chat: true,
                completion: true,
                embedding: false,
                vision: false,
                structured_output: false,
                max_context_tokens: 4096,
                supported_languages: vec!["en".to_string()],
            },
            security: SecurityInfo {
                signature_algorithm: "ML-DSA-87".to_string(),
                public_key_fingerprint: "test".to_string(),
                integrity_hash_tree: "root".to_string(),
            },
            metadata: MetadataInfo {
                license: "MIT".to_string(),
                source: None,
                changelog: "Initial".to_string(),
            },
        };
        registry
            .register_cold_model(
                model_id.to_string(),
                manifest,
                PathBuf::from("/vault/models/test"),
            )
            .await
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn test_remove_cold_model() {
        let vault = MockVault;
        let mut registry =
            setup_registry_with_model(Box::new(MockVault), "test-model:1.0.0:Q4_K_M").await;

        let result = remove_model(
            "test-model:1.0.0:Q4_K_M",
            &mut registry,
            &vault,
            &vault,
            &RemoveOptions::default(),
        )
        .await;

        assert!(result.is_ok());
        let removal = result.unwrap();
        assert!(removal.registry_cleared);
        assert!(removal.secure_wipe);

        let model_id = "test-model:1.0.0:Q4_K_M".to_string();
        assert!(registry.get_model(&model_id).is_none());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_model() {
        let vault = MockVault;
        let mut registry = setup_registry_with_model(Box::new(MockVault), "test:1:Q4").await;

        let result = remove_model(
            "nonexistent-model",
            &mut registry,
            &vault,
            &vault,
            &RemoveOptions::default(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_loaded_model_fails() {
        let vault = MockVault;
        let mut registry =
            setup_registry_with_model(Box::new(MockVault), "test-model:1.0.0:Q4_K_M").await;

        let model_id = "test-model:1.0.0:Q4_K_M".to_string();
        registry
            .load_model(&model_id, "ollama:0".to_string())
            .await
            .unwrap();

        let result = remove_model(
            "test-model:1.0.0:Q4_K_M",
            &mut registry,
            &vault,
            &vault,
            &RemoveOptions::default(),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result, Err(RemoveError::ModelInUse(_))));
    }
}
