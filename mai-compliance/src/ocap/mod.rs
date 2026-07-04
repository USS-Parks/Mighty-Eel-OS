//! OCAP — Ownership, Control, Access, Possession.
//!
//! OCAP is the First Nations data governance framework. Tribal
//! communities own their data, control its use, control access to it,
//! and must physically possess it. For tribal-government deployments,
//! the MAI inference path must enforce that tribal data never leaves
//! the appliance and is only processed under the tribal nation's own
//! governance rules.
//!
//! This module is the compliance-side enforcement layer:
//!
//! - [`tribal_data`] — detects tribal identifiers (treaty references,
//!   reserves, clans, sacred sites, traditional knowledge,
//!   elder-attributed material).
//! - [`treaty`] — recognises treaty references and applies per-treaty
//!   routing rules.
//! - [`cultural`] — flags ceremonial / sacred / elder-attributed
//!   content for human review.
//! - [`ocap_rules`] — the policy engine that combines the three
//!   detection outputs with [`crate::trust::TrustContext`] governance
//!   metadata to produce an [`OcapDecision`].
//!
//! ## TrustContext expectations
//!
//! Per `BUILD-EXECUTION-PLAN-V2-UPDATED.md` §A.13 and
//! `docs/SERVICE-IDENTITY.md`, OCAP must receive a `&TrustContext` on
//! every decision call. OCAP particularly depends on:
//!
//! - `tenant_id` — drives the tenant governance lookup (which tribal
//!   nation, which authorised profiles, which consent registry).
//! - `compliance_scopes` — `ComplianceScope::Ocap` must be present, or
//!   evaluation is refused entirely.
//! - `allowed_routes` — `LocalOnly` is the OCAP default; a cloud route
//!   ceiling violation is itself an OCAP violation.
//! - `revocation_status` — tribal data on a revoked claim is denied.
//! - `trust_bundle_version` and `claim_id` — recorded in every
//!   [`OcapDecision`] so the audit log can correlate the
//!   decision back to the credential event.

pub mod cultural;
pub mod ocap_rules;
pub mod treaty;
pub mod tribal_data;

pub use cultural::{
    CulturalConfidence, CulturalFilter, CulturalFilterConfig, CulturalHit, CulturalReport,
    CulturalSignal,
};
pub use ocap_rules::{
    AccessRole, ConsentStatus, GovernanceMetadata, OcapConfig, OcapDecision, OcapError,
    OcapEvaluator, OcapOutcome, OcapReason, PossessionStatus,
};
pub use treaty::{TreatyDetector, TreatyDetectorConfig, TreatyHit, TreatyId, TreatyReport};
pub use tribal_data::{
    OcapConfidence, TribalDataDetector, TribalDataDetectorConfig, TribalDataReport, TribalHit,
    TribalIdentifierKind,
};
