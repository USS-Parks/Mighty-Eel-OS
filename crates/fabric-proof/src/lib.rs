//! `fabric-proof` — the shared audit-proof primitives for the Sovereignty Stack:
//! canonical-JSON encoding, BLAKE3 hashing + hash chains, HMAC subject
//! pseudonymization, and ML-DSA-87 signed-bundle verification (over
//! [`fabric_crypto`]).
//!
//! Extracted from mai-compliance's proven code so WSF's receipt
//! ledger + trust-bundle verification and mai-compliance share one
//! implementation. The canonical-JSON encoding is **byte-identical** to
//! mai-compliance's, so a bundle hashes the same in both crates.
//! Dependency-light: blake3, hmac, sha2, serde, serde_json, hex, fabric-crypto.

pub mod bundle;
pub mod canonical;
pub mod chain;
pub mod error;
pub mod subject_hash;

pub use bundle::{AcceptAllBundleVerifier, BundleVerifier, MlDsaBundleVerifier};
pub use canonical::{canonical_bytes, canonical_hash, combined_hash, write_canonical};
pub use chain::{ChainLink, GENESIS_HASH, chain_link, verify_chain};
pub use error::ProofError;
pub use subject_hash::{HMAC_PREFIX, MIN_TENANT_KEY_LEN, hmac_subject};
