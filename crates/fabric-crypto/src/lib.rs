//! `fabric-crypto` — the signer/verifier abstraction for the Sovereignty Stack.
//!
//! A trait boundary over post-quantum signing so the substrate is swappable and
//! never a lock-in — the fix for the root problem that MAI called `ml-dsa` /
//! `pqcrypto` directly across 15 files. Two providers today:
//!
//!   * [`providers::RustCryptoMlDsa87`] — pure-Rust ML-DSA-87 (FIPS 204), the
//!     offline/air-gap-capable **default**. Mirrors mai-vault's proven `pqc-dev`
//!     backend, so no new crypto library is introduced.
//!   * [`providers::TransitSigner`] — the OpenBao Transit **custody** backend
//!     (key never leaves the vault). A seam: open-source OpenBao Transit does not
//!     yet expose ML-DSA (only Vault Enterprise 1.19 does, experimentally), so it
//!     fails closed until Phase W lights it up.
//!
//! `fabric-proof` maps the raw signature bytes produced here onto the wire
//! `fabric_contracts::Signature`; this crate stays pure crypto (bytes in / out).

pub mod error;
pub mod providers;

pub use error::CryptoError;

/// ML-DSA-87 (FIPS 204) public-key length in bytes.
pub const MLDSA87_PK_LEN: usize = 2592;
/// ML-DSA-87 (FIPS 204) secret/signing-key length in bytes.
pub const MLDSA87_SK_LEN: usize = 4896;
/// ML-DSA-87 (FIPS 204) signature length in bytes.
pub const MLDSA87_SIG_LEN: usize = 4627;

/// Produces detached signatures over a canonical message.
pub trait Signer: Send + Sync {
    /// The signature algorithm identifier (e.g. `"ml-dsa-87"`).
    fn algorithm(&self) -> &'static str;
    /// The id of the key this signer holds.
    fn key_id(&self) -> &str;
    /// The encoded public key (empty for custody backends that don't expose it).
    fn public_key(&self) -> &[u8];
    /// Sign `message`, returning the raw detached signature bytes.
    ///
    /// # Errors
    /// Returns [`CryptoError`] if the key material is malformed or the provider
    /// is unavailable.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError>;
}

/// Verifies detached signatures. Stateless: the public key is supplied per call.
pub trait Verifier: Send + Sync {
    /// The signature algorithm identifier.
    fn algorithm(&self) -> &'static str;
    /// Verify `signature` over `message` under `public_key`. Wrong-sized inputs
    /// return `Ok(false)`, not an error.
    ///
    /// # Errors
    /// Returns [`CryptoError`] only on an internal provider failure.
    fn verify(
        &self,
        message: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<bool, CryptoError>;
}
