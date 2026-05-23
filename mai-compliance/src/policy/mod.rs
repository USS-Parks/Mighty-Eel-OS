//! Policy runtime support (Session 41).
//!
//! Currently exposes only the [`bundle`] submodule, which defines the
//! unified decision input fed into the composer (landing later in S41).

pub mod bundle;

pub use bundle::{ClassificationResult, PolicyBundle, PolicyBundleError, RequestMetadata};
