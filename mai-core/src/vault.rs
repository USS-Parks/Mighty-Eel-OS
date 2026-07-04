//! # Vault Interface (L2)
//!
//! Trait surface for the IM-OS vault layer. The MAI does not own the vault;
//! it defines typed interfaces that the concrete `mai-vault` crate implements.
//!
//! ## Subsystems
//!
//! - **ZFS vault**: encrypted model storage, integrity verification, snapshots
//! - **PQC cryptography**: ML-KEM (Kyber-1024) encryption, ML-DSA (Dilithium5) signatures
//! - **TPM 2.0**: hardware key sealing/unsealing, attestation
//! - **Profile store**: family profile CRUD in encrypted SQLite
//! - **Audit store**: hash-chained, PQC-signed append-only audit trail
//! - **Vector store**: Qdrant embedding storage and similarity search
//!
//! ## Air-Gap Safety
//!
//! Every trait method must work with zero network access. The Qdrant instance
//! is local (127.0.0.1). No trait method may initiate outbound connections.
//!
//! ## Backward Compatibility
//!
//! The original `VaultInterface` trait (4 methods) is preserved for existing
//! consumers (registry, hotswap, test mocks). New subsystem traits are added
//! alongside it. The concrete `mai-vault` crate implements all traits.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Error Types
// ============================================================================

/// Vault errors covering all L2 subsystems.
///
/// Existing code that calls `.to_string()` on `VaultError` is
/// unaffected by new variants.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    // -- ZFS / storage (original) --
    #[error("Model not found in vault: {0}")]
    ModelNotFound(String),
    #[error("Signature verification failed")]
    SignatureInvalid,
    #[error("Vault I/O error: {0}")]
    IoError(String),
    #[error("Encryption error: {0}")]
    EncryptionError(String),

    // -- ZFS / storage (new) --
    #[error("Model already exists in vault: {0}")]
    ModelAlreadyExists(String),
    #[error("Insufficient vault storage: need {needed} bytes, have {available} bytes")]
    InsufficientStorage { needed: u64, available: u64 },
    #[error("Snapshot not found: {0}")]
    SnapshotNotFound(String),
    #[error("ZFS operation failed: {0}")]
    ZfsError(String),

    // -- Integrity --
    #[error("Integrity check failed: expected {expected}, got {actual}")]
    IntegrityCheckFailed { expected: String, actual: String },

    // -- Cryptography --
    #[error("Decryption error: {0}")]
    DecryptionError(String),
    #[error("Key generation error: {0}")]
    KeyGenerationError(String),
    #[error("PQC operation failed: {0}")]
    PqcError(String),

    // -- TPM --
    #[error("TPM not available")]
    TpmUnavailable,
    #[error("TPM operation failed: {0}")]
    TpmError(String),
    #[error("Key sealed to different PCR state")]
    TpmPcrMismatch,

    // -- Profile store --
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),
    #[error("Profile already exists: {0}")]
    ProfileAlreadyExists(String),
    #[error("Profile store error: {0}")]
    ProfileStoreError(String),

    // -- Audit --
    #[error("Audit chain broken at entry {index}: {detail}")]
    AuditChainBroken { index: u64, detail: String },
    #[error("Audit store error: {0}")]
    AuditStoreError(String),

    // -- Vector store --
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),
    #[error("Collection already exists: {0}")]
    CollectionAlreadyExists(String),
    #[error("Vector store error: {0}")]
    VectorStoreError(String),
    #[error("Embedding dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    // -- General --
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError::IoError(e.to_string())
    }
}

// ============================================================================
// Original VaultInterface (UNCHANGED - backward compatible)
// ============================================================================

/// Original vault interface consumed by mai-core modules (registry, health, hotswap).
///
/// This trait is preserved for backward compatibility with existing consumers.
/// New code should prefer the specific subsystem traits below.
#[async_trait]
pub trait VaultInterface: Send + Sync {
    /// Retrieve decrypted model weights from vault storage.
    async fn load_model_weights(&self, model_id: &str) -> Result<Vec<u8>, VaultError>;

    /// Store model package in vault (post-signature-verification).
    async fn store_model_package(&self, model_id: &str, data: &[u8]) -> Result<(), VaultError>;

    /// Append an entry to the tamper-evident audit trail.
    async fn append_audit_entry(&self, entry: &[u8]) -> Result<(), VaultError>;

    /// Verify PQC signature on a model package.
    async fn verify_signature(&self, data: &[u8], signature: &[u8]) -> Result<bool, VaultError>;
}

// ============================================================================
// Data Types: Storage
// ============================================================================

/// Information about vault storage capacity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInfo {
    /// Total vault capacity in bytes.
    pub total_bytes: u64,
    /// Currently used bytes.
    pub used_bytes: u64,
    /// Available bytes for new models.
    pub available_bytes: u64,
    /// Number of model packages stored.
    pub model_count: u32,
    /// ZFS compression ratio (e.g., 1.5 = 50% space savings).
    pub compression_ratio: f64,
}

/// ZFS snapshot metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Snapshot name (auto-generated or user-provided).
    pub name: String,
    /// Unix timestamp of snapshot creation.
    pub created_at: u64,
    /// Size of data referenced by this snapshot (bytes).
    pub referenced_bytes: u64,
    /// Human-readable reason for the snapshot.
    pub reason: String,
}

/// Model integrity verification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityResult {
    /// Whether the model passed integrity checks.
    pub valid: bool,
    /// SHA-256 hash of the model weights on disk.
    pub computed_hash: String,
    /// Expected hash from the model manifest.
    pub expected_hash: String,
    /// Size of the verified data in bytes.
    pub verified_bytes: u64,
}

// ============================================================================
// Data Types: Profiles
// ============================================================================

/// A family profile stored in the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FamilyProfile {
    /// Unique profile identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Role determining base permissions.
    pub role: ProfileRole,
    /// Models this profile is allowed to use (empty = all allowed for role).
    pub model_access: Vec<String>,
    /// Priority level for request scheduling (1 = highest, 10 = lowest).
    pub priority_level: u8,
    /// Content filter level (0 = none, 1 = mild, 2 = moderate, 3 = strict).
    pub content_filter_level: u8,
    /// Maximum tokens per day (0 = unlimited).
    pub daily_token_limit: u64,
    /// Maximum concurrent requests.
    pub max_concurrent_requests: u8,
    /// Unix timestamp of profile creation.
    pub created_at: u64,
    /// Unix timestamp of last activity.
    pub last_active: u64,
    /// Whether the profile is currently active.
    pub active: bool,
}

/// Profile roles with hierarchical permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProfileRole {
    /// Full system access: all models, configuration, audit, power control.
    Admin,
    /// Standard access: all models appropriate for adults, usage history.
    Adult,
    /// Teen access: filtered models with content safety, own history.
    Teen,
    /// Restricted access: child-safe models only, no system operations.
    Child,
    /// Minimal access: default models only, no history, rate-limited.
    Guest,
}

impl ProfileRole {
    /// Permission set for this role.
    pub fn permissions(&self) -> ProfilePermissions {
        match self {
            Self::Admin => ProfilePermissions {
                can_inference: true,
                can_manage_models: true,
                can_view_audit: true,
                can_manage_profiles: true,
                can_control_power: true,
                can_access_system: true,
                can_export_compliance: true,
                can_manage_vectors: true,
            },
            Self::Adult => ProfilePermissions {
                can_inference: true,
                can_manage_models: false,
                can_view_audit: true,
                can_manage_profiles: false,
                can_control_power: false,
                can_access_system: false,
                can_export_compliance: false,
                can_manage_vectors: true,
            },
            Self::Teen => ProfilePermissions {
                can_inference: true,
                can_manage_models: false,
                can_view_audit: false,
                can_manage_profiles: false,
                can_control_power: false,
                can_access_system: false,
                can_export_compliance: false,
                can_manage_vectors: true,
            },
            Self::Child | Self::Guest => ProfilePermissions {
                can_inference: true,
                can_manage_models: false,
                can_view_audit: false,
                can_manage_profiles: false,
                can_control_power: false,
                can_access_system: false,
                can_export_compliance: false,
                can_manage_vectors: false,
            },
        }
    }
}

/// Capability flags derived from profile role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePermissions {
    pub can_inference: bool,
    pub can_manage_models: bool,
    pub can_view_audit: bool,
    pub can_manage_profiles: bool,
    pub can_control_power: bool,
    pub can_access_system: bool,
    pub can_export_compliance: bool,
    pub can_manage_vectors: bool,
}

/// Notification emitted when a profile changes.
#[derive(Debug, Clone)]
pub enum ProfileChangeEvent {
    Created(String),
    Updated(String),
    Deleted(String),
    RoleChanged {
        profile_id: String,
        old_role: ProfileRole,
        new_role: ProfileRole,
    },
}

// ============================================================================
// Data Types: Audit
// ============================================================================

/// A vault-level audit entry for the L2 audit trail.
///
/// Distinct from the API-level `AuditEntry` in mai-api. This covers
/// storage operations, encryption events, profile changes, and
/// compliance-relevant actions at the vault layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultAuditEntry {
    /// Unique entry identifier.
    pub entry_id: String,
    /// Unix timestamp (seconds).
    pub timestamp: u64,
    /// Profile that initiated the action.
    pub profile_id: String,
    /// Action category.
    pub action: VaultAuditAction,
    /// Model involved (if applicable).
    pub model_id: Option<String>,
    /// Input token count (if inference-related).
    pub tokens_in: Option<u64>,
    /// Output token count (if inference-related).
    pub tokens_out: Option<u64>,
    /// Operation latency in milliseconds.
    pub latency_ms: u64,
    /// Adapter used (opaque identifier, never exposed to users).
    pub adapter_id: Option<String>,
    /// Operation status.
    pub status: VaultAuditStatus,
    /// Error code if status is Error.
    pub error_code: Option<String>,
    /// Source IP (always 127.0.0.1 for air-gapped systems).
    pub ip_source: String,
    /// Hash of the previous entry (hex SHA3-256).
    pub previous_hash: String,
    /// Hash of this entry (hex SHA3-256).
    pub entry_hash: String,
    /// PQC signature over entry_hash (hex ML-DSA, optional).
    pub pqc_signature: Option<String>,
}

/// Categories of auditable vault actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VaultAuditAction {
    ModelLoad,
    ModelUnload,
    ModelInstall,
    ModelRemove,
    ModelIntegrityCheck,
    EncryptionOp,
    DecryptionOp,
    KeyGeneration,
    KeySeal,
    KeyUnseal,
    ProfileCreate,
    ProfileUpdate,
    ProfileDelete,
    AuditExport,
    SnapshotCreate,
    SnapshotRollback,
    SnapshotDelete,
    VectorCollectionCreate,
    VectorCollectionDelete,
    VectorSearch,
    VectorBackup,
    SystemStartup,
    SystemShutdown,
}

/// Status of an audited vault operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VaultAuditStatus {
    Success,
    Error,
    Denied,
}

/// Compliance export report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    /// Report generation timestamp (Unix seconds).
    pub generated_at: u64,
    /// Time range start (Unix seconds).
    pub range_start: u64,
    /// Time range end (Unix seconds).
    pub range_end: u64,
    /// Total entries in range.
    pub total_entries: u64,
    /// Entries grouped by action category.
    pub action_summary: HashMap<String, u64>,
    /// Entries grouped by profile.
    pub profile_summary: HashMap<String, u64>,
    /// Chain integrity verified up to this entry.
    pub chain_verified_to: u64,
    /// Whether the chain is intact.
    pub chain_intact: bool,
    /// PQC signature over the report hash.
    pub report_signature: Option<String>,
}

// ============================================================================
// Data Types: Vector Store
// ============================================================================

/// Configuration for a vector collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    /// Collection name (unique per family profile context).
    pub name: String,
    /// Embedding vector dimension (must match model output).
    pub dimension: usize,
    /// Distance metric for similarity search.
    pub distance: DistanceMetric,
    /// Owning profile ID (collections are profile-scoped).
    pub profile_id: String,
}

/// Distance metrics for vector similarity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
}

/// A vector with its associated metadata payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingPoint {
    /// Unique point identifier.
    pub id: String,
    /// The embedding vector.
    pub vector: Vec<f32>,
    /// Arbitrary metadata payload (JSON-compatible).
    pub payload: HashMap<String, serde_json::Value>,
}

/// Result from a similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Point identifier.
    pub id: String,
    /// Similarity score (interpretation depends on distance metric).
    pub score: f32,
    /// Metadata payload from the matched point.
    pub payload: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Data Types: PQC Key Management
// ============================================================================

/// Key hierarchy level in the vault encryption scheme.
///
/// ```text
/// master_key (TPM-sealed)
///   -> model_encryption_key (per-model, derived from master)
///     -> file_key (per-weight-file, derived from model key)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyLevel {
    Master,
    ModelEncryption,
    PerFile,
}

/// Metadata about a managed encryption key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    /// Key identifier.
    pub key_id: String,
    /// Level in the key hierarchy.
    pub level: KeyLevel,
    /// Algorithm (e.g., "ML-KEM-1024", "ML-DSA-87").
    pub algorithm: String,
    /// Unix timestamp of key creation.
    pub created_at: u64,
    /// Associated model ID (for model/file keys).
    pub model_id: Option<String>,
    /// Whether the key is sealed in TPM.
    pub tpm_sealed: bool,
}

// ============================================================================
// Trait: ModelStorage (Extended ZFS operations)
// ============================================================================

/// Extended model storage operations on encrypted ZFS.
///
/// Supplements `VaultInterface` with integrity verification, space reporting,
/// snapshot management, and secure deletion. The concrete `mai-vault` crate
/// implements both `VaultInterface` and `ModelStorage`.
#[async_trait]
pub trait ModelStorage: Send + Sync {
    /// Verify integrity of stored model weights against manifest hash.
    async fn verify_model_integrity(&self, model_id: &str) -> Result<IntegrityResult, VaultError>;

    /// Report available vault storage capacity.
    async fn storage_info(&self) -> Result<StorageInfo, VaultError>;

    /// Securely remove a model from the vault (ZFS scrub + overwrite).
    async fn remove_model(&self, model_id: &str) -> Result<(), VaultError>;

    /// Create a pre-update snapshot for rollback capability.
    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError>;

    /// Roll back to a named snapshot, discarding changes since.
    async fn rollback_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError>;

    /// Delete a snapshot that is no longer needed.
    async fn delete_snapshot(&self, snapshot_name: &str) -> Result<(), VaultError>;

    /// List all available snapshots.
    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, VaultError>;

    /// Check if a model exists in the vault.
    async fn model_exists(&self, model_id: &str) -> Result<bool, VaultError>;

    /// Get the on-disk size of a stored model (bytes).
    async fn model_size(&self, model_id: &str) -> Result<u64, VaultError>;
}

// ============================================================================
// Trait: PqcProvider (Post-Quantum Cryptography)
// ============================================================================

/// Post-quantum cryptography operations.
///
/// Uses ML-KEM-1024 (FIPS 203, formerly Kyber) for key encapsulation and
/// ML-DSA-87 (FIPS 204, formerly Dilithium5) for digital signatures.
/// Both are NIST PQC standards finalized in 2024.
#[async_trait]
pub trait PqcProvider: Send + Sync {
    // -- ML-KEM key encapsulation --

    /// Generate an ML-KEM-1024 keypair. Returns (public_key, secret_key).
    async fn kem_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError>;

    /// Encapsulate: produce (ciphertext, shared_secret) from a public key.
    async fn kem_encapsulate(&self, public_key: &[u8]) -> Result<(Vec<u8>, Vec<u8>), VaultError>;

    /// Decapsulate: recover shared_secret from ciphertext + secret key.
    async fn kem_decapsulate(
        &self,
        ciphertext: &[u8],
        secret_key: &[u8],
    ) -> Result<Vec<u8>, VaultError>;

    // -- ML-DSA digital signatures --

    /// Generate an ML-DSA-87 signing keypair. Returns (public_key, signing_key).
    async fn dsa_generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>), VaultError>;

    /// Sign data with an ML-DSA-87 signing key.
    async fn dsa_sign(&self, data: &[u8], signing_key: &[u8]) -> Result<Vec<u8>, VaultError>;

    /// Verify an ML-DSA-87 signature against data and public key.
    async fn dsa_verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<bool, VaultError>;

    // -- High-level operations --

    /// Encrypt model weights using the model's encryption key.
    /// Uses ML-KEM to wrap a symmetric key, then AES-256-GCM for bulk data.
    async fn encrypt_model_weights(
        &self,
        model_id: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, VaultError>;

    /// Decrypt model weights using the model's encryption key.
    async fn decrypt_model_weights(
        &self,
        model_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, VaultError>;

    /// Sign a model package for distribution/verification.
    async fn sign_package(&self, package_data: &[u8]) -> Result<Vec<u8>, VaultError>;

    /// Verify a model package signature.
    async fn verify_package(
        &self,
        package_data: &[u8],
        signature: &[u8],
    ) -> Result<bool, VaultError>;
}

// ============================================================================
// Trait: TpmProvider (TPM 2.0 Key Management)
// ============================================================================

/// TPM 2.0 hardware key management.
///
/// Keys are sealed to the current PCR (Platform Configuration Register) state.
/// If the system firmware or boot chain changes, sealed keys become inaccessible
/// until re-sealed by an admin.
#[async_trait]
pub trait TpmProvider: Send + Sync {
    /// Check if a TPM 2.0 device is available and functional.
    async fn is_available(&self) -> bool;

    /// Seal a key to the current PCR state. Returns sealed blob.
    async fn seal_key(&self, key_data: &[u8], key_id: &str) -> Result<Vec<u8>, VaultError>;

    /// Unseal a key. Fails if PCR state has changed since sealing.
    async fn unseal_key(&self, sealed_blob: &[u8], key_id: &str) -> Result<Vec<u8>, VaultError>;

    /// Get a TPM attestation quote for the current system state.
    async fn get_attestation(&self) -> Result<Vec<u8>, VaultError>;

    /// List all key IDs currently sealed in the TPM.
    async fn list_sealed_keys(&self) -> Result<Vec<KeyInfo>, VaultError>;

    /// Remove a sealed key from the TPM.
    async fn remove_sealed_key(&self, key_id: &str) -> Result<(), VaultError>;
}

// ============================================================================
// Trait: ProfileStore (Family Profiles)
// ============================================================================

/// Encrypted SQLite-backed family profile store.
///
/// Profiles are stored in an encrypted SQLite database within the vault.
/// All profile operations are atomic and crash-safe (WAL mode).
#[async_trait]
pub trait ProfileStore: Send + Sync {
    /// Retrieve a profile by ID.
    async fn get_profile(&self, profile_id: &str) -> Result<FamilyProfile, VaultError>;

    /// List all profiles, optionally filtered by role.
    async fn list_profiles(
        &self,
        role_filter: Option<ProfileRole>,
    ) -> Result<Vec<FamilyProfile>, VaultError>;

    /// Create a new profile. Fails if the ID already exists.
    async fn create_profile(&self, profile: &FamilyProfile) -> Result<(), VaultError>;

    /// Update an existing profile. Fails if not found.
    async fn update_profile(&self, profile: &FamilyProfile) -> Result<(), VaultError>;

    /// Delete a profile by ID. Fails if not found.
    async fn delete_profile(&self, profile_id: &str) -> Result<(), VaultError>;

    /// Map a profile to its permission set (role-based + overrides).
    async fn get_permissions(&self, profile_id: &str) -> Result<ProfilePermissions, VaultError>;

    /// Update the last_active timestamp for a profile.
    async fn touch_activity(&self, profile_id: &str) -> Result<(), VaultError>;

    /// Get the total number of profiles.
    async fn profile_count(&self) -> Result<u32, VaultError>;
}

// ============================================================================
// Trait: AuditStore (Vault-Level Audit Trail)
// ============================================================================

/// Append-only, hash-chained, PQC-signed audit trail in the vault.
///
/// Uses SQLite WAL mode for crash safety. Each entry is hash-chained to
/// the previous entry. Periodic entries are PQC-signed with ML-DSA.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Append an audit entry, extending the hash chain.
    async fn append(&self, entry: &VaultAuditEntry) -> Result<(), VaultError>;

    /// Read the most recent N entries.
    async fn read_recent(&self, count: usize) -> Result<Vec<VaultAuditEntry>, VaultError>;

    /// Read entries for a specific profile (most recent first).
    async fn read_by_profile(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<VaultAuditEntry>, VaultError>;

    /// Read entries within a time range (Unix seconds).
    async fn read_by_time_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<VaultAuditEntry>, VaultError>;

    /// Verify the integrity of the audit chain from entry 0 to the latest.
    /// Returns Ok(count) or Err with the index and detail of the break.
    async fn verify_chain(&self) -> Result<u64, VaultError>;

    /// Generate a compliance report for a time range.
    async fn export_compliance(&self, start: u64, end: u64)
    -> Result<ComplianceReport, VaultError>;

    /// Get the total entry count.
    async fn entry_count(&self) -> Result<u64, VaultError>;

    /// Get the hash of the most recent entry.
    async fn last_hash(&self) -> Result<String, VaultError>;
}

// ============================================================================
// Trait: VectorStore (Qdrant Integration)
// ============================================================================

/// Local Qdrant vector database interface for RAG embedding storage.
///
/// Collections are scoped to family profiles. The Qdrant instance runs
/// locally (127.0.0.1:6334) and is never exposed to the network.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Create a new embedding collection.
    async fn create_collection(&self, config: &CollectionConfig) -> Result<(), VaultError>;

    /// Delete a collection and all its data.
    async fn delete_collection(&self, collection_name: &str) -> Result<(), VaultError>;

    /// List all collections, optionally filtered by profile.
    async fn list_collections(
        &self,
        profile_filter: Option<&str>,
    ) -> Result<Vec<CollectionConfig>, VaultError>;

    /// Store embedding points in a collection.
    async fn store_embeddings(
        &self,
        collection_name: &str,
        points: &[EmbeddingPoint],
    ) -> Result<(), VaultError>;

    /// Search for similar vectors. Returns top-k results by score.
    async fn search_similar(
        &self,
        collection_name: &str,
        query_vector: &[f32],
        top_k: usize,
        score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>, VaultError>;

    /// Delete specific points from a collection.
    async fn delete_points(
        &self,
        collection_name: &str,
        point_ids: &[String],
    ) -> Result<(), VaultError>;

    /// Get the number of points in a collection.
    async fn point_count(&self, collection_name: &str) -> Result<u64, VaultError>;

    /// Snapshot the Qdrant data to the ZFS vault for backup.
    async fn backup_to_vault(&self) -> Result<String, VaultError>;

    /// Restore Qdrant data from a vault backup.
    async fn restore_from_vault(&self, backup_id: &str) -> Result<(), VaultError>;
}

// ============================================================================
// Composite Trait
// ============================================================================

/// Convenience super-trait combining all vault capabilities.
///
/// Implementations that provide all L2 services implement this.
/// Individual traits can also be used independently for testing.
pub trait FullVault:
    VaultInterface + ModelStorage + PqcProvider + TpmProvider + ProfileStore + AuditStore + VectorStore
{
}

/// Blanket implementation: anything implementing all sub-traits is a FullVault.
impl<T> FullVault for T where
    T: VaultInterface
        + ModelStorage
        + PqcProvider
        + TpmProvider
        + ProfileStore
        + AuditStore
        + VectorStore
{
}
