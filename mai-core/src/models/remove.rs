use std::collections::HashMap;

use thiserror::Error;
use tracing::{info, warn};

use crate::registry::ModelEntry;
use crate::vault::{ModelStorage, VaultInterface};

/// Options for model removal
#[derive(Debug, Clone)]
pub struct RemoveOptions {
    /// Whether to create a pre-removal ZFS snapshot. Note (V7): a snapshot
    /// **retains** the model's encrypted blocks until the snapshot itself is
    /// destroyed — crypto-erasure (below) is what makes those retained blocks
    /// unreadable.
    pub create_snapshot: bool,
    /// Whether to cryptographically erase the model — retire its encryption
    /// key so the at-rest ciphertext (on disk and in any retained snapshot)
    /// is permanently unrecoverable. This replaces the former
    /// "secure_overwrite_passes" field: on copy-on-write ZFS, overwriting
    /// blocks in place does not destroy the originals, so pass-count
    /// overwriting was never a real guarantee.
    pub crypto_erase: bool,
}

impl Default for RemoveOptions {
    fn default() -> Self {
        Self {
            create_snapshot: true,
            crypto_erase: true,
        }
    }
}

/// Removal result
#[derive(Debug, Clone)]
pub struct RemovalResult {
    /// Model ID that was removed
    pub model_id: String,
    /// Whether the model's encryption key was retired (cryptographic
    /// erasure). `false` when the model held no per-model key (e.g. a legacy
    /// plaintext store) — the caller should treat such models as not
    /// confidentially erased.
    pub crypto_erased: bool,
    /// Whether registry entry was deleted
    pub registry_cleared: bool,
    /// Whether pre-removal snapshot was created. When `true`, the model's
    /// (encrypted) blocks are retained by that snapshot until it is destroyed.
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
pub(crate) async fn remove_model(
    model_id: &str,
    models: &mut HashMap<String, ModelEntry>,
    vault: &dyn VaultInterface,
    storage: Option<&dyn ModelStorage>,
    options: &RemoveOptions,
) -> Result<RemovalResult, RemoveError> {
    // Check model can be removed (must not be loaded)
    let status = models.get(model_id).map(|e| &e.status).ok_or_else(|| {
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

    // Create pre-removal snapshot (if storage is available)
    if options.create_snapshot
        && let Some(storage) = storage
    {
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

    // Remove from vault. V7: the vault's `remove_model` cryptographically
    // erases the model (retires its encryption key) — the real deletion
    // guarantee on copy-on-write storage — and then unlinks the artifact.
    let mut crypto_erased = false;
    if options.crypto_erase {
        if let Some(storage) = storage {
            info!(model_id, "Cryptographically erasing model from vault");
            storage.remove_model(model_id).await.map_err(|e| {
                RemoveError::VaultError(format!("Failed to remove model from vault: {e}"))
            })?;
            crypto_erased = true;
        } else {
            info!(model_id, "No ModelStorage attached, skipping vault removal");
        }
    }

    // Remove from registry
    models.remove(model_id);

    // Write audit entry
    let audit_entry = format!(
        "{{\"event\":\"model_removed\",\"model_id\":\"{model_id}\",\"crypto_erased\":{crypto_erased}, \"snapshot\":{snapshot_created}}}",
    );
    if let Err(e) = vault.append_audit_entry(audit_entry.as_bytes()).await {
        warn!(error = %e, "Failed to write audit entry for model removal");
    }

    info!(model_id, crypto_erased, "Model removed successfully");

    Ok(RemovalResult {
        model_id: model_id.to_string(),
        crypto_erased,
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

        let vault_ref: &dyn VaultInterface = &vault;
        let storage_ref: Option<&dyn ModelStorage> = Some(&vault);
        let result = remove_model(
            "test-model:1.0.0:Q4_K_M",
            &mut registry.models,
            vault_ref,
            storage_ref,
            &RemoveOptions::default(),
        )
        .await;

        assert!(result.is_ok());
        let removal = result.unwrap();
        assert!(removal.registry_cleared);
        assert!(removal.crypto_erased);

        let model_id = "test-model:1.0.0:Q4_K_M".to_string();
        assert!(!registry.models.contains_key(&model_id));
    }

    #[tokio::test]
    async fn test_remove_nonexistent_model() {
        let vault = MockVault;
        let mut registry = setup_registry_with_model(Box::new(MockVault), "test:1:Q4").await;

        let vault_ref: &dyn VaultInterface = &vault;
        let storage_ref: Option<&dyn ModelStorage> = Some(&vault);
        let result = remove_model(
            "nonexistent-model",
            &mut registry.models,
            vault_ref,
            storage_ref,
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

        let vault_ref: &dyn VaultInterface = &vault;
        let storage_ref: Option<&dyn ModelStorage> = Some(&vault);
        let result = remove_model(
            "test-model:1.0.0:Q4_K_M",
            &mut registry.models,
            vault_ref,
            storage_ref,
            &RemoveOptions::default(),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result, Err(RemoveError::ModelInUse(_))));
    }
}
