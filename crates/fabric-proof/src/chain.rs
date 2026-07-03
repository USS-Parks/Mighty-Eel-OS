//! BLAKE3 hash chain. Each link extends the previous by hashing
//! `previous_hash || entry_hash`. Tamper-evident: [`verify_chain`] walks the
//! links and flags the first break. This is the primitive WSF's receipt ledger
//! builds on ([`fabric_contracts`-style] receipts carry `previous_hash` +
//! `request_hash`). It is `fabric-proof`'s own chain, distinct from
//! mai-compliance's audit-log chain (which stays in that crate).

use crate::error::ProofError;

/// The genesis previous-hash the first link in a chain must extend (all zeroes).
pub const GENESIS_HASH: [u8; 32] = [0u8; 32];

/// Combine a previous chain hash with an entry hash into the next chain hash.
#[must_use]
pub fn chain_link(previous_hash: &[u8; 32], entry_hash: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(previous_hash);
    hasher.update(entry_hash);
    *hasher.finalize().as_bytes()
}

/// One node's linkage: the content hash of the entry and the previous chain hash
/// it claims to extend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainLink {
    /// The chain hash this entry extends (`GENESIS_HASH` for the first entry).
    pub previous_hash: [u8; 32],
    /// BLAKE3 hash of this entry's canonical content.
    pub entry_hash: [u8; 32],
}

/// Verify a sequence of links forms an unbroken chain from [`GENESIS_HASH`].
///
/// Returns the final chain hash on success.
///
/// # Errors
/// Returns [`ProofError::ChainBroken`] with the index of the first link whose
/// `previous_hash` does not match the running chain hash.
pub fn verify_chain(links: &[ChainLink]) -> Result<[u8; 32], ProofError> {
    let mut expected = GENESIS_HASH;
    for (i, link) in links.iter().enumerate() {
        if link.previous_hash != expected {
            return Err(ProofError::ChainBroken {
                index: i,
                detail: "previous_hash does not match the prior link".to_string(),
            });
        }
        expected = chain_link(&link.previous_hash, &link.entry_hash);
    }
    Ok(expected)
}
