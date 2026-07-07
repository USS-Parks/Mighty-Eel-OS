//! `fabric-contracts` — the four frozen v1 wire schemas of the Sovereignty Stack
//! (WSF + AOG). Pure types only: no crypto, no I/O, no policy logic. Signing and
//! hash-chaining live in `fabric-proof`; attenuation enforcement lives in
//! `fabric-token`. This crate is the single source of truth both products depend
//! on so their wire formats can never silently diverge.
//!
//! Specs: `contracts/{identity,trust-token,receipt,envelope}.md`.

pub mod common;
pub mod envelope;
pub mod identity;
pub mod receipt;
pub mod token;

pub use common::{
    Classification, ComplianceScope, RevocationStatus, Route, RoutingDecision, Signature,
};
pub use envelope::{Envelope, Label, Seal, Thread};
pub use identity::{Identity, IdentityKind, WsfPrincipal};
pub use receipt::{Correlation, PeriodicSignature, Receipt};
pub use token::{Attenuation, Budget, Caveat, CaveatType, TrustToken};
