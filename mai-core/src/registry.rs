//! Model Registry - Manifest management and lifecycle tracking
//!
//! Manages model metadata, storage locations, integrity verification, and
//! state transitions. Supports air-gap-safe updates via USB.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{AdapterId, ModelId};
use crate::vault::VaultInterface;

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
}

/// Model registry main struct
pub struct ModelRegistry {
    models: HashMap<ModelId, ModelEntry>,
    vault: Box<dyn VaultInterface>,
}

struct ModelEntry {
    manifest: ModelManifest,
    status: ModelStatus,
    vault_path: PathBuf,
    loaded_adapter: Option<AdapterId>,
    loaded_gpu: Option<String>,
}

impl ModelRegistry {
    /// Create new registry with vault interface
    pub fn new(vault: Box<dyn VaultInterface>) -> Self {
        Self {
            models: HashMap::new(),
            vault,
        }
    }

    /// Parse and validate a model manifest from TOML
    pub fn parse_manifest(toml_content: &str) -> Result<ModelManifest, RegistryError> {
        // Implementation in Session 07
        todo!()
    }

    /// Register a model in cold storage (after integrity verification)
    pub async fn register_cold_model(
        &mut self,
        model_id: ModelId,
        manifest: ModelManifest,
        vault_path: PathBuf,
    ) -> Result<(), RegistryError> {
        // Implementation in Session 07
        todo!()
    }

    /// Load a model from vault to VRAM via adapter
    pub async fn load_model(
        &mut self,
        model_id: &ModelId,
        target_adapter: AdapterId,
    ) -> Result<(), RegistryError> {
        // Implementation in Session 07
        todo!()
    }

    /// Unload a model from VRAM back to cold storage
    pub async fn unload_model(&mut self, model_id: &ModelId) -> Result<(), RegistryError> {
        // Implementation in Session 07
        todo!()
    }

    /// Install model from USB drive (air-gap safe)
    pub async fn install_from_usb(
        &mut self,
        usb_mount_point: &Path,
        package_name: &str,
    ) -> Result<InstallResult, RegistryError> {
        // Implementation in Session 07
        todo!()
    }

    /// Get model entry by ID
    pub fn get_model(&self, model_id: &ModelId) -> Option<&ModelManifest> {
        self.models.get(model_id).map(|e| &e.manifest)
    }

    /// List available models with optional filtering
    pub fn list_models(&self, filter: Option<&ModelFilter>) -> Vec<ModelSummary> {
        // Implementation in Session 07
        todo!()
    }

    /// Update model status (called by scheduler/power state machine)
    pub fn update_status(
        &mut self,
        model_id: &ModelId,
        new_status: ModelStatus,
    ) -> Result<(), RegistryError> {
        // Implementation in Session 07
        todo!()
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
    /// Model capabilities
    pub capabilities: CapabilityInfo,
}
