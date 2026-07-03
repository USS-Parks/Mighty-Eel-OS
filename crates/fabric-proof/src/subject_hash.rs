//! HMAC-SHA256 subject pseudonymization (BF-3 §6).
//!
//! `subject_hash = "hmac:" + lowercase_hex(HMAC-SHA256(tenant_key, subject_id))`.
//! String-based core — mai-compliance wraps this with its `SubjectId` /
//! `SubjectHash` newtypes. The `tenant_key` never leaves the appliance; a
//! different key produces a different hash for the same subject, so cross-tenant
//! correlation is impossible.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::ProofError;

/// Required minimum length of the per-tenant HMAC key, in bytes. SHA-256 outputs
/// 32 bytes, so a shorter key gives no security benefit.
pub const MIN_TENANT_KEY_LEN: usize = 32;

/// The prefix every HMAC-pseudonymized identifier begins with, distinguishing it
/// from a raw identifier.
pub const HMAC_PREFIX: &str = "hmac:";

/// Compute the HMAC-SHA256 pseudonym of `subject_id` under `tenant_key`.
///
/// # Errors
/// Returns [`ProofError::TenantKeyTooShort`] when `tenant_key` is shorter than
/// [`MIN_TENANT_KEY_LEN`].
pub fn hmac_subject(tenant_key: &[u8], subject_id: &str) -> Result<String, ProofError> {
    if tenant_key.len() < MIN_TENANT_KEY_LEN {
        return Err(ProofError::TenantKeyTooShort {
            got: tenant_key.len(),
            min: MIN_TENANT_KEY_LEN,
        });
    }
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(tenant_key).expect("HMAC accepts any key length");
    mac.update(subject_id.as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut out = String::with_capacity(HMAC_PREFIX.len() + bytes.len() * 2);
    out.push_str(HMAC_PREFIX);
    out.push_str(&hex::encode(bytes));
    Ok(out)
}
