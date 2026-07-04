//! Model Registry - Manifest management and lifecycle tracking
//!
//! Manages model metadata, storage locations, integrity verification, and
//! state transitions. Supports air-gap-safe updates via USB.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::types::{AdapterId, ModelId};
use crate::vault::{ModelStorage, VaultError, VaultInterface};

/// Model manifest schema (matches TOML structure)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Core model info
    pub model: ModelInfo,
    /// Compatibility requirements
    pub compatibility: CompatibilityInfo,
    /// Logical capabilities
    pub capabilities: CapabilityInfo,
    /// PQC security metadata
    pub security: SecurityInfo,
    /// Human-readable metadata
    pub metadata: MetadataInfo,
}

/// Core model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model name (e.g., "qwen3-14b")
    pub name: String,
    /// Semantic version
    pub version: String,
    /// Weight file format
    pub format: ModelFormat,
    /// Quantization level (e.g., "Q4_K_M")
    pub quantization: Option<String>,
    /// Total size on disk in bytes
    pub size_bytes: u64,
    /// VRAM required including overhead
    pub required_vram_bytes: u64,
}

/// Supported model weight formats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelFormat {
    /// llama.cpp quantized format
    GGUF,
    /// HuggingFace safe format
    SafeTensors,
    /// ExLlamaV2 quantized format
    EXL2,
    /// GPTQ quantized format
    GPTQ,
}

/// Backend and hardware compatibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityInfo {
    /// Minimum MAI version required
    pub min_mai_version: String,
    /// Compatible backend adapters
    pub supported_backends: Vec<String>,
    /// Compatible hardware classes
    pub hardware_classes: Vec<String>,
}

/// Logical model capabilities (no hardware details)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInfo {
    /// Supports multi-turn chat
    pub chat: bool,
    /// Supports single-shot completion
    pub completion: bool,
    /// Supports embedding computation
    pub embedding: bool,
    /// Supports vision/multimodal inputs
    pub vision: bool,
    /// Supports structured/constrained output
    pub structured_output: bool,
    /// Maximum context window in tokens
    pub max_context_tokens: u32,
    /// Supported natural languages
    pub supported_languages: Vec<String>,
}

/// PQC security metadata for air-gap integrity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityInfo {
    /// Signature algorithm (e.g., "ML-DSA-87")
    pub signature_algorithm: String,
    /// Public key fingerprint for verification
    pub public_key_fingerprint: String,
    /// Merkle tree root hash for integrity
    pub integrity_hash_tree: String,
}

/// Human-readable model metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataInfo {
    /// License identifier
    pub license: String,
    /// Source URL (informational only, not fetched)
    pub source: Option<String>,
    /// Changelog entry for this version
    pub changelog: String,
}

/// Model lifecycle states
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    /// Model package stored in encrypted ZFS vault, not loaded
    ColdStorage,
    /// Manifest verified, weights being decrypted and loaded to VRAM
    Loading { progress_percent: u8 },
    /// Loaded in VRAM, ready to serve requests
    Loaded,
    /// Actively serving requests (tracked for LRU eviction)
    Active {
        last_used: Instant,
        request_count: u64,
    },
    /// Being evicted: finishing in-flight requests, freeing VRAM
    Evicting,
    /// Removed from VRAM, returned to ColdStorage
    Evicted,
}

impl ModelStatus {
    /// Check if a transition from self to target is valid per state machine rules.
    /// Valid transitions:
    ///   ColdStorage -> Loading
    ///   Loading -> Loaded
    ///   Loading -> ColdStorage (load failure rollback)
    ///   Loaded -> Active
    ///   Active -> Loaded (idle, no longer serving)
    ///   Active -> Evicting
    ///   Loaded -> Evicting
    ///   Evicting -> Evicted
    ///   Evicted -> ColdStorage (reset cycle)
    fn is_valid_transition(&self, target: &ModelStatus) -> bool {
        matches!(
            (self, target),
            (ModelStatus::ColdStorage, ModelStatus::Loading { .. })
                | (
                    ModelStatus::Loading { .. } | ModelStatus::Active { .. },
                    ModelStatus::Loaded
                )
                | (
                    ModelStatus::Loading { .. } | ModelStatus::Evicted,
                    ModelStatus::ColdStorage
                )
                | (
                    ModelStatus::Loaded,
                    ModelStatus::Active { .. } | ModelStatus::Evicting
                )
                | (ModelStatus::Active { .. }, ModelStatus::Evicting)
                | (ModelStatus::Evicting, ModelStatus::Evicted)
        )
    }

    /// Short display name for logging
    fn display_name(&self) -> &'static str {
        match self {
            Self::ColdStorage => "ColdStorage",
            Self::Loading { .. } => "Loading",
            Self::Loaded => "Loaded",
            Self::Active { .. } => "Active",
            Self::Evicting => "Evicting",
            Self::Evicted => "Evicted",
        }
    }
}

/// Registry errors
#[derive(Error, Debug)]
pub enum RegistryError {
    /// Model not found in registry
    #[error("Model {0} not found")]
    ModelNotFound(String),

    /// Manifest parsing or validation failed
    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),

    /// PQC signature does not verify
    #[error("Signature verification failed: {0}")]
    SignatureVerificationFailed(String),

    /// Merkle hash tree integrity check failed
    #[error("Integrity check failed: expected {expected}, got {actual}")]
    IntegrityCheckFailed { expected: String, actual: String },

    /// Not enough VRAM to load model
    #[error("Insufficient VRAM: required {required}, available {available}")]
    InsufficientVram { required: u64, available: u64 },

    /// No compatible backend adapter for this model
    #[error("Incompatible backend: {0}")]
    IncompatibleBackend(String),

    /// Vault layer error
    #[error("Vault error: {0}")]
    VaultError(String),

    /// Invalid state transition
    #[error("Invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    /// Model already registered
    #[error("Model already registered: {0}")]
    AlreadyRegistered(String),

    /// USB package not found or unreadable
    #[error("USB package error: {0}")]
    UsbPackageError(String),

    /// Model removal failed
    #[error("Model removal failed: {0}")]
    RemovalFailed(String),
}

impl From<VaultError> for RegistryError {
    fn from(e: VaultError) -> Self {
        RegistryError::VaultError(e.to_string())
    }
}

/// Model registry main struct
pub struct ModelRegistry {
    pub(crate) models: HashMap<ModelId, ModelEntry>,
    vault: Box<dyn VaultInterface>,
    storage: Option<Box<dyn ModelStorage>>,
}

/// Internal entry tracking model state and location
pub(crate) struct ModelEntry {
    pub(crate) manifest: ModelManifest,
    pub(crate) status: ModelStatus,
    pub(crate) vault_path: PathBuf,
    pub(crate) loaded_adapter: Option<AdapterId>,
    pub(crate) loaded_gpu: Option<String>,
}

impl ModelRegistry {
    /// Create new registry with vault interface
    pub fn new(vault: Box<dyn VaultInterface>) -> Self {
        Self {
            models: HashMap::new(),
            vault,
            storage: None,
        }
    }

    /// Attach a ModelStorage implementation for extended operations
    pub fn with_storage(mut self, storage: Box<dyn ModelStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Parse and validate a model manifest from TOML string.
    /// Validates required fields and internal consistency.
    pub fn parse_manifest(toml_content: &str) -> Result<ModelManifest, RegistryError> {
        let manifest: ModelManifest = toml::from_str(toml_content)
            .map_err(|e| RegistryError::InvalidManifest(e.to_string()))?;

        // Validate required fields are non-empty
        if manifest.model.name.is_empty() {
            return Err(RegistryError::InvalidManifest(
                "model.name cannot be empty".to_string(),
            ));
        }
        if manifest.model.version.is_empty() {
            return Err(RegistryError::InvalidManifest(
                "model.version cannot be empty".to_string(),
            ));
        }
        if manifest.model.size_bytes == 0 {
            return Err(RegistryError::InvalidManifest(
                "model.size_bytes must be > 0".to_string(),
            ));
        }
        if manifest.model.required_vram_bytes == 0 {
            return Err(RegistryError::InvalidManifest(
                "model.required_vram_bytes must be > 0".to_string(),
            ));
        }
        if manifest.compatibility.supported_backends.is_empty() {
            return Err(RegistryError::InvalidManifest(
                "compatibility.supported_backends cannot be empty".to_string(),
            ));
        }
        if manifest.security.integrity_hash_tree.is_empty() {
            return Err(RegistryError::InvalidManifest(
                "security.integrity_hash_tree cannot be empty".to_string(),
            ));
        }

        debug!(
            model = %manifest.model.name,
            version = %manifest.model.version,
            "Parsed model manifest"
        );

        Ok(manifest)
    }

    /// Register a model in cold storage (after integrity verification).
    /// The model package must already exist in the vault at `vault_path`.
    #[allow(clippy::unused_async)] // async for API consistency with vault operations
    pub async fn register_cold_model(
        &mut self,
        model_id: ModelId,
        manifest: ModelManifest,
        vault_path: PathBuf,
    ) -> Result<(), RegistryError> {
        if self.models.contains_key(&model_id) {
            return Err(RegistryError::AlreadyRegistered(model_id));
        }

        info!(
            model_id = %model_id,
            name = %manifest.model.name,
            version = %manifest.model.version,
            size_bytes = manifest.model.size_bytes,
            "Registering model in cold storage"
        );

        let entry = ModelEntry {
            manifest,
            status: ModelStatus::ColdStorage,
            vault_path,
            loaded_adapter: None,
            loaded_gpu: None,
        };
        self.models.insert(model_id, entry);
        Ok(())
    }

    /// Load a model from vault to VRAM via the specified adapter.
    /// Transitions: ColdStorage -> Loading -> Loaded
    pub async fn load_model(
        &mut self,
        model_id: &ModelId,
        target_adapter: AdapterId,
    ) -> Result<(), RegistryError> {
        // Verify model exists and is in ColdStorage
        let entry = self
            .models
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.clone()))?;

        if !entry.status.is_valid_transition(&ModelStatus::Loading {
            progress_percent: 0,
        }) {
            return Err(RegistryError::InvalidTransition {
                from: entry.status.display_name().to_string(),
                to: "Loading".to_string(),
            });
        }

        info!(
            model_id = %model_id,
            adapter = %target_adapter,
            "Loading model from vault"
        );

        // Transition to Loading
        entry.status = ModelStatus::Loading {
            progress_percent: 0,
        };
        entry.loaded_adapter = Some(target_adapter.clone());

        // Request model weights from vault (PQC decryption happens inside vault)
        let weights_result = self.vault.load_model_weights(model_id).await;

        let entry = self
            .models
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.clone()))?;

        match weights_result {
            Ok(_weights) => {
                // Weights loaded successfully.
                // TODO(basho): pass weights to the adapter process for VRAM
                // placement via HIL; currently just marks the entry Loaded.
                entry.status = ModelStatus::Loaded;
                info!(model_id = %model_id, "Model loaded successfully");
                Ok(())
            }
            Err(e) => {
                // Load failed, rollback to ColdStorage
                warn!(
                    model_id = %model_id,
                    error = %e,
                    "Model load failed, rolling back to ColdStorage"
                );
                entry.status = ModelStatus::ColdStorage;
                entry.loaded_adapter = None;
                Err(RegistryError::VaultError(e.to_string()))
            }
        }
    }

    /// Unload a model from VRAM back to cold storage.
    /// Valid from Loaded or Active states. Transitions through Evicting -> Evicted -> ColdStorage.
    pub fn unload_model(&mut self, model_id: &ModelId) -> Result<(), RegistryError> {
        let entry = self
            .models
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.clone()))?;

        // Must be in Loaded or Active to unload
        let can_evict = matches!(
            entry.status,
            ModelStatus::Loaded | ModelStatus::Active { .. }
        );
        if !can_evict {
            return Err(RegistryError::InvalidTransition {
                from: entry.status.display_name().to_string(),
                to: "Evicting".to_string(),
            });
        }

        info!(model_id = %model_id, "Unloading model");

        // Transition through eviction states
        entry.status = ModelStatus::Evicting;
        // In production: drain in-flight requests, free VRAM via HIL MemoryManager
        entry.status = ModelStatus::Evicted;
        // Reset to cold storage
        entry.status = ModelStatus::ColdStorage;
        entry.loaded_adapter = None;
        entry.loaded_gpu = None;

        info!(model_id = %model_id, "Model unloaded to cold storage");
        Ok(())
    }

    /// Install model from USB drive (air-gap safe).
    /// Delegates to `models::install::install_package` for canonical logic.
    pub async fn install_from_usb(
        &mut self,
        usb_mount_point: &Path,
        package_name: &str,
    ) -> Result<InstallResult, RegistryError> {
        use crate::models::package::ModelPackage;

        let package_dir = usb_mount_point.join(package_name);
        let pkg = ModelPackage::open(&package_dir).map_err(|e| {
            RegistryError::UsbPackageError(format!(
                "Cannot open package at {}: {e}",
                package_dir.display(),
            ))
        })?;

        let current_version = env!("CARGO_PKG_VERSION");
        let &mut Self {
            ref mut models,
            ref vault,
            ref storage,
        } = self;

        crate::models::install::install_package(
            &pkg,
            models,
            &**vault as &dyn VaultInterface,
            storage.as_deref(),
            current_version,
            None,
        )
        .await
    }

    /// Get model manifest by ID
    pub fn get_model(&self, model_id: &ModelId) -> Option<&ModelManifest> {
        self.models.get(model_id).map(|e| &e.manifest)
    }

    /// Get model status by ID
    pub fn get_status(&self, model_id: &ModelId) -> Option<&ModelStatus> {
        self.models.get(model_id).map(|e| &e.status)
    }

    /// List available models with optional filtering
    pub fn list_models(&self, filter: Option<&ModelFilter>) -> Vec<ModelSummary> {
        self.models
            .iter()
            .filter(|(_, entry)| {
                if let Some(f) = filter {
                    // Filter by backend compatibility
                    if let Some(ref backend) = f.backend_compatible_with
                        && !entry
                            .manifest
                            .compatibility
                            .supported_backends
                            .contains(backend)
                    {
                        return false;
                    }
                    // Filter by capability
                    if let Some(ref cap) = f.requires_capability {
                        let has_cap = match cap.as_str() {
                            "chat" => entry.manifest.capabilities.chat,
                            "completion" => entry.manifest.capabilities.completion,
                            "embedding" => entry.manifest.capabilities.embedding,
                            "vision" => entry.manifest.capabilities.vision,
                            "structured_output" => entry.manifest.capabilities.structured_output,
                            _ => false,
                        };
                        if !has_cap {
                            return false;
                        }
                    }
                    // Filter by VRAM budget
                    if let Some(max_vram) = f.max_vram_bytes
                        && entry.manifest.model.required_vram_bytes > max_vram
                    {
                        return false;
                    }
                }
                true
            })
            .map(|(model_id, entry)| ModelSummary {
                model_id: model_id.clone(),
                name: entry.manifest.model.name.clone(),
                version: entry.manifest.model.version.clone(),
                status: entry.status.clone(),
                size_bytes: entry.manifest.model.size_bytes,
                required_vram_bytes: entry.manifest.model.required_vram_bytes,
                capabilities: entry.manifest.capabilities.clone(),
            })
            .collect()
    }

    /// Securely remove a model from the vault and registry.
    /// Delegates to `models::remove::remove_model` for canonical logic.
    pub async fn secure_remove_model(
        &mut self,
        model_id: &str,
        options: &super::models::remove::RemoveOptions,
    ) -> Result<super::models::remove::RemovalResult, RegistryError> {
        let &mut Self {
            ref mut models,
            ref vault,
            ref storage,
            ..
        } = self;

        crate::models::remove::remove_model(
            model_id,
            models,
            &**vault as &dyn VaultInterface,
            storage.as_deref(),
            options,
        )
        .await
        .map_err(|e| match e {
            super::models::remove::RemoveError::ModelInUse(id) => {
                RegistryError::RemovalFailed(format!("Model {id} is currently loaded"))
            }
            super::models::remove::RemoveError::VaultError(msg) => {
                RegistryError::RemovalFailed(msg)
            }
            super::models::remove::RemoveError::RegistryError(msg) => {
                RegistryError::RemovalFailed(msg)
            }
        })
    }

    /// Remove a model from the registry (must be in ColdStorage or Evicted).
    pub fn remove_model_entry(&mut self, model_id: &str) -> Result<(), RegistryError> {
        let entry = self
            .models
            .get(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))?;

        let can_remove = matches!(
            entry.status,
            ModelStatus::ColdStorage | ModelStatus::Evicted
        );
        if !can_remove {
            return Err(RegistryError::RemovalFailed(format!(
                "Model {model_id} is in state {} and cannot be removed",
                entry.status.display_name()
            )));
        }

        self.models.remove(model_id);
        info!(model_id, "Model removed from registry");
        Ok(())
    }

    /// Update model status with transition validation.
    /// Enforces valid state machine transitions.
    pub fn update_status(
        &mut self,
        model_id: &ModelId,
        new_status: ModelStatus,
    ) -> Result<(), RegistryError> {
        let entry = self
            .models
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.clone()))?;

        if !entry.status.is_valid_transition(&new_status) {
            return Err(RegistryError::InvalidTransition {
                from: entry.status.display_name().to_string(),
                to: new_status.display_name().to_string(),
            });
        }

        debug!(
            model_id = %model_id,
            from = entry.status.display_name(),
            to = new_status.display_name(),
            "Model status transition"
        );

        entry.status = new_status;
        Ok(())
    }

    /// Find models that can serve a given request type and fit in available VRAM.
    /// Used by scheduler for capability matching.
    pub fn find_capable_models(&self, capability: &str, available_vram: u64) -> Vec<ModelId> {
        self.models
            .iter()
            .filter(|(_, entry)| {
                // Must be loaded or active
                let is_available = matches!(
                    entry.status,
                    ModelStatus::Loaded | ModelStatus::Active { .. }
                );
                if !is_available {
                    return false;
                }
                // Must have the requested capability
                let has_cap = match capability {
                    "chat" => entry.manifest.capabilities.chat,
                    "completion" => entry.manifest.capabilities.completion,
                    "embedding" => entry.manifest.capabilities.embedding,
                    "vision" => entry.manifest.capabilities.vision,
                    "structured_output" => entry.manifest.capabilities.structured_output,
                    _ => false,
                };
                if !has_cap {
                    return false;
                }
                // Must fit in available VRAM (already loaded, so check passes)
                entry.manifest.model.required_vram_bytes <= available_vram
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get the adapter currently serving a model
    pub fn get_loaded_adapter(&self, model_id: &ModelId) -> Option<&AdapterId> {
        self.models
            .get(model_id)
            .and_then(|e| e.loaded_adapter.as_ref())
    }

    /// Mark a model as actively serving (Loaded -> Active)
    pub fn mark_active(&mut self, model_id: &ModelId) -> Result<(), RegistryError> {
        let entry = self
            .models
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.clone()))?;

        match &entry.status {
            ModelStatus::Loaded => {
                entry.status = ModelStatus::Active {
                    last_used: Instant::now(),
                    request_count: 1,
                };
                Ok(())
            }
            ModelStatus::Active { request_count, .. } => {
                entry.status = ModelStatus::Active {
                    last_used: Instant::now(),
                    request_count: request_count + 1,
                };
                Ok(())
            }
            _ => Err(RegistryError::InvalidTransition {
                from: entry.status.display_name().to_string(),
                to: "Active".to_string(),
            }),
        }
    }

    /// Find the least recently used active model for eviction
    pub fn find_lru_model(&self) -> Option<ModelId> {
        self.models
            .iter()
            .filter_map(|(id, entry)| {
                if let ModelStatus::Active { last_used, .. } = &entry.status {
                    Some((id.clone(), *last_used))
                } else if matches!(entry.status, ModelStatus::Loaded) {
                    // Loaded but not active is higher eviction priority
                    Some((id.clone(), Instant::now()))
                } else {
                    None
                }
            })
            .min_by_key(|(_, last_used)| *last_used)
            .map(|(id, _)| id)
    }

    /// Total number of registered models
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Number of currently loaded models (Loaded + Active)
    pub fn loaded_count(&self) -> usize {
        self.models
            .values()
            .filter(|e| matches!(e.status, ModelStatus::Loaded | ModelStatus::Active { .. }))
            .count()
    }
}

/// Result of USB installation
#[derive(Debug)]
pub struct InstallResult {
    /// Installed model ID
    pub model_id: ModelId,
    /// When installation completed
    pub installed_at: Instant,
    /// Whether integrity hash tree verified
    pub integrity_verified: bool,
    /// Whether PQC signature verified
    pub signature_verified: bool,
}

/// Filter options for model listing
#[derive(Debug, Clone)]
pub struct ModelFilter {
    /// Only models compatible with this backend
    pub backend_compatible_with: Option<String>,
    /// Only models with this capability
    pub requires_capability: Option<String>,
    /// Only models fitting within this VRAM budget
    pub max_vram_bytes: Option<u64>,
}

/// Summary for model listing
#[derive(Debug, Clone)]
pub struct ModelSummary {
    /// Model identifier
    pub model_id: ModelId,
    /// Model name
    pub name: String,
    /// Model version
    pub version: String,
    /// Current lifecycle status
    pub status: ModelStatus,
    /// Size on disk
    pub size_bytes: u64,
    /// VRAM required for inference
    pub required_vram_bytes: u64,
    /// Model capabilities
    pub capabilities: CapabilityInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock vault for testing
    struct MockVault {
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl VaultInterface for MockVault {
        async fn load_model_weights(&self, model_id: &str) -> Result<Vec<u8>, VaultError> {
            if self.should_fail {
                Err(VaultError::ModelNotFound(model_id.to_string()))
            } else {
                Ok(vec![0u8; 1024]) // fake weights
            }
        }
        async fn store_model_package(
            &self,
            _model_id: &str,
            _data: &[u8],
        ) -> Result<(), VaultError> {
            if self.should_fail {
                Err(VaultError::IoError("mock failure".to_string()))
            } else {
                Ok(())
            }
        }
        async fn append_audit_entry(&self, _entry: &[u8]) -> Result<(), VaultError> {
            Ok(())
        }
        async fn verify_signature(
            &self,
            _data: &[u8],
            _signature: &[u8],
        ) -> Result<bool, VaultError> {
            Ok(!self.should_fail)
        }
    }

    fn test_manifest_toml() -> &'static str {
        r#"
[model]
name = "qwen3-14b"
version = "1.0.0"
format = "GGUF"
quantization = "Q4_K_M"
size_bytes = 8_000_000_000
required_vram_bytes = 10_000_000_000

[compatibility]
min_mai_version = "0.1.0"
supported_backends = ["ollama", "llamacpp"]
hardware_classes = ["nvidia", "cpu"]

[capabilities]
chat = true
completion = true
embedding = false
vision = false
structured_output = true
max_context_tokens = 32768
supported_languages = ["en", "zh", "ja"]

[security]
signature_algorithm = "ML-DSA-87"
public_key_fingerprint = "sha256:abc123def456"
integrity_hash_tree = "sha256:rootHashExample123"

[metadata]
license = "Apache-2.0"
source = "https://huggingface.co/Qwen/Qwen3-14B-GGUF"
changelog = "Initial quantized release for MAI"
"#
    }

    #[test]
    fn test_parse_valid_manifest() {
        let result = ModelRegistry::parse_manifest(test_manifest_toml());
        assert!(result.is_ok());
        let manifest = result.unwrap();
        assert_eq!(manifest.model.name, "qwen3-14b");
        assert_eq!(manifest.model.version, "1.0.0");
        assert_eq!(manifest.model.size_bytes, 8_000_000_000);
        assert!(manifest.capabilities.chat);
        assert!(!manifest.capabilities.embedding);
        assert_eq!(manifest.compatibility.supported_backends.len(), 2);
    }

    #[test]
    fn test_parse_invalid_manifest_empty_name() {
        let bad_toml = test_manifest_toml().replace("qwen3-14b", "");
        let result = ModelRegistry::parse_manifest(&bad_toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RegistryError::InvalidManifest(_)));
    }

    #[test]
    fn test_parse_malformed_toml() {
        let result = ModelRegistry::parse_manifest("not valid toml {{{");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_register_cold_model() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();

        let result = registry
            .register_cold_model(
                "qwen3:1.0.0:Q4_K_M".to_string(),
                manifest,
                PathBuf::from("/vault/models/qwen3"),
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(registry.model_count(), 1);
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();
        let id = "qwen3:1.0.0:Q4_K_M".to_string();

        registry
            .register_cold_model(id.clone(), manifest.clone(), PathBuf::from("/vault/x"))
            .await
            .unwrap();

        let result = registry
            .register_cold_model(id, manifest, PathBuf::from("/vault/y"))
            .await;
        assert!(matches!(result, Err(RegistryError::AlreadyRegistered(_))));
    }

    #[tokio::test]
    async fn test_load_model_success() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();
        let id = "qwen3:1.0.0:Q4_K_M".to_string();

        registry
            .register_cold_model(id.clone(), manifest, PathBuf::from("/vault/x"))
            .await
            .unwrap();

        let result = registry.load_model(&id, "ollama:0".to_string()).await;
        assert!(result.is_ok());
        assert!(matches!(
            registry.get_status(&id),
            Some(ModelStatus::Loaded)
        ));
    }

    #[tokio::test]
    async fn test_load_model_vault_failure_rollback() {
        let vault = Box::new(MockVault { should_fail: true });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();
        let id = "qwen3:1.0.0:Q4_K_M".to_string();

        // Manually insert in cold storage since register doesn't touch vault
        registry.models.insert(
            id.clone(),
            ModelEntry {
                manifest,
                status: ModelStatus::ColdStorage,
                vault_path: PathBuf::from("/vault/x"),
                loaded_adapter: None,
                loaded_gpu: None,
            },
        );

        let result = registry.load_model(&id, "ollama:0".to_string()).await;
        assert!(result.is_err());
        // Should have rolled back to ColdStorage
        assert!(matches!(
            registry.get_status(&id),
            Some(ModelStatus::ColdStorage)
        ));
    }

    #[tokio::test]
    async fn test_unload_model() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();
        let id = "qwen3:1.0.0:Q4_K_M".to_string();

        registry
            .register_cold_model(id.clone(), manifest, PathBuf::from("/vault/x"))
            .await
            .unwrap();
        registry
            .load_model(&id, "ollama:0".to_string())
            .await
            .unwrap();

        let result = registry.unload_model(&id);
        assert!(result.is_ok());
        assert!(matches!(
            registry.get_status(&id),
            Some(ModelStatus::ColdStorage)
        ));
    }

    #[test]
    fn test_state_machine_valid_transitions() {
        let cold = ModelStatus::ColdStorage;
        let loading = ModelStatus::Loading {
            progress_percent: 50,
        };
        let loaded = ModelStatus::Loaded;
        let active = ModelStatus::Active {
            last_used: Instant::now(),
            request_count: 1,
        };
        let evicting = ModelStatus::Evicting;
        let evicted = ModelStatus::Evicted;

        assert!(cold.is_valid_transition(&ModelStatus::Loading {
            progress_percent: 0
        }));
        assert!(loading.is_valid_transition(&ModelStatus::Loaded));
        assert!(loading.is_valid_transition(&ModelStatus::ColdStorage));
        assert!(loaded.is_valid_transition(&ModelStatus::Active {
            last_used: Instant::now(),
            request_count: 0,
        }));
        assert!(active.is_valid_transition(&ModelStatus::Loaded));
        assert!(active.is_valid_transition(&ModelStatus::Evicting));
        assert!(loaded.is_valid_transition(&ModelStatus::Evicting));
        assert!(evicting.is_valid_transition(&ModelStatus::Evicted));
        assert!(evicted.is_valid_transition(&ModelStatus::ColdStorage));
    }

    #[test]
    fn test_state_machine_invalid_transitions() {
        let cold = ModelStatus::ColdStorage;
        let loaded = ModelStatus::Loaded;
        let evicted = ModelStatus::Evicted;

        // Can't jump from cold to loaded
        assert!(!cold.is_valid_transition(&ModelStatus::Loaded));
        // Can't jump from loaded to evicted
        assert!(!loaded.is_valid_transition(&ModelStatus::Evicted));
        // Can't go from evicted to loaded
        assert!(!evicted.is_valid_transition(&ModelStatus::Loaded));
    }

    #[tokio::test]
    async fn test_list_models_with_filter() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();

        registry
            .register_cold_model("m1".to_string(), manifest.clone(), PathBuf::from("/v/m1"))
            .await
            .unwrap();

        // No filter returns all
        let all = registry.list_models(None);
        assert_eq!(all.len(), 1);

        // Filter by compatible backend
        let filter = ModelFilter {
            backend_compatible_with: Some("ollama".to_string()),
            requires_capability: None,
            max_vram_bytes: None,
        };
        let filtered = registry.list_models(Some(&filter));
        assert_eq!(filtered.len(), 1);

        // Filter by incompatible backend
        let filter = ModelFilter {
            backend_compatible_with: Some("tensorrt".to_string()),
            requires_capability: None,
            max_vram_bytes: None,
        };
        let filtered = registry.list_models(Some(&filter));
        assert_eq!(filtered.len(), 0);

        // Filter by VRAM budget too small
        let filter = ModelFilter {
            backend_compatible_with: None,
            requires_capability: None,
            max_vram_bytes: Some(1_000_000), // way too small
        };
        let filtered = registry.list_models(Some(&filter));
        assert_eq!(filtered.len(), 0);
    }

    #[tokio::test]
    async fn test_mark_active_and_find_capable() {
        let vault = Box::new(MockVault { should_fail: false });
        let mut registry = ModelRegistry::new(vault);
        let manifest = ModelRegistry::parse_manifest(test_manifest_toml()).unwrap();
        let id = "qwen3:1.0.0:Q4_K_M".to_string();

        registry
            .register_cold_model(id.clone(), manifest, PathBuf::from("/v/x"))
            .await
            .unwrap();
        registry
            .load_model(&id, "ollama:0".to_string())
            .await
            .unwrap();
        registry.mark_active(&id).unwrap();

        // Should find this model for chat capability
        let capable = registry.find_capable_models("chat", u64::MAX);
        assert_eq!(capable.len(), 1);
        assert_eq!(capable[0], id);

        // Should not find for embedding (model doesn't support it)
        let capable = registry.find_capable_models("embedding", u64::MAX);
        assert_eq!(capable.len(), 0);
    }
}
