//! # `SecureLoadContext` Trait
//!
//! TPM-attested model loading, encrypted model weight transfer from
//! vault, and integrity verification before inference begins.
//!
//! ## Contract
//!
//! - `attest()` verifies TPM PCR state matches expected boot chain
//! - `unseal_key(key_id)` retrieves encryption key from TPM
//! - `verify_integrity(model_path, expected_hash)` checks SHA-256 hash tree
//! - `decrypt_weights(encrypted, key)` decrypts model weights via ML-KEM
//! - Attestation failure is a hard error: model load is refused

// Stub: trait definition in Session 02, implementation in Session 06
