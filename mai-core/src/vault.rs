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

// Stub: implementation in Session 12
