//! HMAC-SHA256 subject pseudonymization (BF-3 §6).
//!
//! Raw subject identifiers (employee IDs, patient MRNs, treaty signatories,
//! service accounts) cannot appear in audit logs that may sync to a central
//! audit store. This module provides the per-tenant HMAC construction that
//! turns a raw [`crate::trust::SubjectId`] into a stable
//! [`crate::trust::SubjectHash`] suitable for audit correlation.
//!
//! ## Construction
//!
//! ```text
//! subject_hash = "hmac:" + lowercase_hex(HMAC-SHA256(tenant_key, subject_id))
//! ```
//!
//! The `tenant_key` is held in the local vault and never leaves the
//! appliance. The `"hmac:"` prefix marks the string as a pseudonymized
//! identifier in audit records; raw subject IDs MUST NEVER appear without
//! this prefix.
//!
//! Cross-tenant correlation is intentionally impossible: a different
//! tenant key produces a different hash for the same subject id.

use thiserror::Error;

use crate::trust::{SubjectHash, SubjectId};

/// Required minimum length of the per-tenant HMAC key, in bytes. Single-sourced
/// from `fabric-proof`, which owns the HMAC construction (SOV-F1).
pub const MIN_TENANT_KEY_LEN: usize = fabric_proof::MIN_TENANT_KEY_LEN;

/// The prefix every HMAC-pseudonymized identifier begins with. The presence of
/// this prefix is how audit consumers distinguish a pseudonymized identifier
/// from a raw one.
pub const HMAC_PREFIX: &str = fabric_proof::HMAC_PREFIX;

/// Errors produced when constructing a [`SubjectHash`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SubjectHashError {
    /// The per-tenant HMAC key is shorter than [`MIN_TENANT_KEY_LEN`].
    #[error("tenant HMAC key too short: {got} bytes (minimum {min})")]
    TenantKeyTooShort { got: usize, min: usize },
}

/// Compute the HMAC-SHA256 pseudonym of `subject_id` under `tenant_key`.
///
/// Delegates the construction to [`fabric_proof::hmac_subject`] (SOV-F1) and
/// wraps the result in the crate-local [`SubjectHash`] newtype. Returns a hash
/// whose inner string begins with the [`HMAC_PREFIX`] marker.
///
/// # Errors
///
/// Returns [`SubjectHashError::TenantKeyTooShort`] when `tenant_key` is
/// shorter than [`MIN_TENANT_KEY_LEN`] (32 bytes).
pub fn hmac_subject(
    tenant_key: &[u8],
    subject_id: &SubjectId,
) -> Result<SubjectHash, SubjectHashError> {
    match fabric_proof::hmac_subject(tenant_key, subject_id.as_str()) {
        Ok(s) => Ok(SubjectHash::new(s)),
        Err(fabric_proof::ProofError::TenantKeyTooShort { got, min }) => {
            Err(SubjectHashError::TenantKeyTooShort { got, min })
        }
        Err(_) => unreachable!("fabric_proof::hmac_subject only returns TenantKeyTooShort"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> Vec<u8> {
        // 32 deterministic bytes per seed for test stability.
        (0..32u8).map(|i| i.wrapping_add(seed)).collect()
    }

    #[test]
    fn same_subject_and_key_yields_same_hash() {
        let k = key(1);
        let subj = SubjectId::new("user-42");
        let a = hmac_subject(&k, &subj).unwrap();
        let b = hmac_subject(&k, &subj).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_subject_same_key_yields_different_hash() {
        let k = key(1);
        let a = hmac_subject(&k, &SubjectId::new("user-42")).unwrap();
        let b = hmac_subject(&k, &SubjectId::new("user-43")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn different_key_same_subject_yields_different_hash() {
        let subj = SubjectId::new("user-42");
        let a = hmac_subject(&key(1), &subj).unwrap();
        let b = hmac_subject(&key(2), &subj).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn output_starts_with_hmac_prefix() {
        let h = hmac_subject(&key(1), &SubjectId::new("user-42")).unwrap();
        assert!(h.as_str().starts_with("hmac:"));
    }

    #[test]
    fn output_is_lowercase_hex_after_prefix() {
        let h = hmac_subject(&key(1), &SubjectId::new("user-42")).unwrap();
        let hex_part = h.as_str().strip_prefix("hmac:").unwrap();
        assert_eq!(hex_part.len(), 64); // 32 bytes * 2 hex chars
        assert!(
            hex_part
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn short_key_rejected() {
        let short = vec![0u8; 16];
        let err = hmac_subject(&short, &SubjectId::new("x")).unwrap_err();
        assert_eq!(
            err,
            SubjectHashError::TenantKeyTooShort {
                got: 16,
                min: MIN_TENANT_KEY_LEN
            }
        );
    }

    #[test]
    fn minimum_length_key_accepted() {
        let exact = vec![0u8; MIN_TENANT_KEY_LEN];
        let h = hmac_subject(&exact, &SubjectId::new("x")).unwrap();
        assert!(h.as_str().starts_with("hmac:"));
    }

    #[test]
    fn empty_subject_id_hashes_deterministically() {
        // Empty SubjectId is unusual but must not panic and must produce
        // a stable result across calls.
        let k = key(7);
        let a = hmac_subject(&k, &SubjectId::new("")).unwrap();
        let b = hmac_subject(&k, &SubjectId::new("")).unwrap();
        assert_eq!(a, b);
    }
}
