//! Vault configuration.
//!
//! Loaded from TOML config on disk. Defines paths, encryption settings,
//! and connection parameters for all L2 subsystems.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level vault configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// ZFS storage configuration.
    pub storage: StorageConfig,
    /// PQC encryption configuration.
    pub pqc: PqcConfig,
    /// TPM configuration.
    pub tpm: TpmConfig,
    /// Profile store configuration.
    pub profiles: ProfileStoreConfig,
    /// Audit trail configuration.
    pub audit: AuditConfig,
    /// Vector store configuration.
    pub vectors: VectorConfig,
}

/// ZFS dataset and storage paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// ZFS dataset name (e.g., "im-vault/models").
    pub dataset: String,
    /// Mount point for model weights.
    pub mount_point: PathBuf,
    /// Staging directory for incoming packages.
    pub staging_dir: PathBuf,
    /// Maximum vault capacity in bytes (0 = use ZFS quota).
    pub max_capacity_bytes: u64,
    /// Enable ZFS compression (lz4).
    pub compression_enabled: bool,
}

/// Post-quantum cryptography settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqcConfig {
    /// KEM algorithm identifier (default: "ML-KEM-1024").
    pub kem_algorithm: String,
    /// DSA algorithm identifier (default: "ML-DSA-87").
    pub dsa_algorithm: String,
    /// Key storage directory (within vault).
    pub key_store_path: PathBuf,
    /// Symmetric cipher for bulk data (default: "AES-256-GCM").
    pub symmetric_cipher: String,
}

/// TPM 2.0 hardware settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TpmConfig {
    /// TPM device path (e.g., "/dev/tpmrm0").
    pub device_path: String,
    /// Whether to require TPM for key operations (false = software fallback).
    pub required: bool,
    /// PCR indices to bind sealed keys to.
    pub pcr_indices: Vec<u32>,
}

/// SQLite profile store settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStoreConfig {
    /// Path to the encrypted SQLite database.
    pub db_path: PathBuf,
    /// Enable WAL mode for crash safety.
    pub wal_mode: bool,
    /// Maximum number of profiles (0 = unlimited).
    pub max_profiles: u32,
}

/// Audit trail settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Path to the audit SQLite database.
    pub db_path: PathBuf,
    /// Enable WAL mode for crash safety.
    pub wal_mode: bool,
    /// How often to PQC-sign an entry (e.g., every 100 entries).
    pub sign_interval: u64,
    /// Maximum entries before rotation (0 = no rotation).
    pub max_entries: u64,
}

/// Qdrant vector database settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    /// Qdrant gRPC endpoint (default: "http://127.0.0.1:6334").
    pub endpoint: String,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Request timeout in seconds.
    pub request_timeout_secs: u64,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            storage: StorageConfig {
                dataset: "im-vault/models".into(),
                mount_point: PathBuf::from("/vault/models"),
                staging_dir: PathBuf::from("/vault/staging"),
                max_capacity_bytes: 0,
                compression_enabled: true,
            },
            pqc: PqcConfig {
                kem_algorithm: "ML-KEM-1024".into(),
                dsa_algorithm: "ML-DSA-87".into(),
                key_store_path: PathBuf::from("/vault/keys"),
                symmetric_cipher: "AES-256-GCM".into(),
            },
            tpm: TpmConfig {
                device_path: "/dev/tpmrm0".into(),
                required: false,
                pcr_indices: vec![0, 7],
            },
            profiles: ProfileStoreConfig {
                db_path: PathBuf::from("/vault/profiles.db"),
                wal_mode: true,
                max_profiles: 0,
            },
            audit: AuditConfig {
                db_path: PathBuf::from("/vault/audit.db"),
                wal_mode: true,
                sign_interval: 100,
                max_entries: 0,
            },
            vectors: VectorConfig {
                endpoint: "http://127.0.0.1:6334".into(),
                connect_timeout_secs: 5,
                request_timeout_secs: 30,
            },
        }
    }
}
