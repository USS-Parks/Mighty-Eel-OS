//! # Vault Interface
//!
//! Abstraction over L2 vault operations. The MAI does not implement
//! the vault. It provides a typed interface for API server and agent
//! layer to request vault operations.
//!
//! ## Operations
//!
//! - Model weight storage and retrieval (ZFS datasets)
//! - PQC encryption/decryption (ML-KEM key encapsulation, ML-DSA signatures)
//! - TPM 2.0 key seal/unseal
//! - Family profile CRUD (`SQLite`)
//! - Audit trail append (hash-chained, tamper-evident)
//! - `Qdrant` vector DB operations (embedding storage, similarity search)
//! - Compliance audit data export

use async_trait::async_trait;

/// Vault interface consumed by mai-core modules (registry, health, hotswap).
/// Full implementation in Session 12.
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

/// Vault errors (minimal for Session 04 stub; expanded in Session 12)
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("Model not found in vault: {0}")]
    ModelNotFound(String),
    #[error("Signature verification failed")]
    SignatureInvalid,
    #[error("Vault I/O error: {0}")]
    IoError(String),
    #[error("Encryption error: {0}")]
    EncryptionError(String),
}

// Full implementation in Session 12
