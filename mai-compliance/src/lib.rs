//! MAI Compliance Engine.
//!
//! HIPAA-specific modules (Session 38):
//!
//! - [`phi`] — detector for all 18 HIPAA Safe Harbor identifiers with
//!   Explicit / Probable / Possible confidence levels.
//! - [`baa`] — Business Associate Agreement enforcement engine with
//!   Standard / Strict / Custom modes.
//! - [`deid`] — PHI redaction with re-identification risk scoring.
//! - [`medical_entities`] — ICD-10 code validation, RxNorm medication
//!   dictionary, and lab-value parsing that enrich routing decisions.
//!
//! Export-control modules (Session 39):
//!
//! - [`itar`] — USML category I–XXI detection with the
//!   default-to-ITAR-on-ambiguity rule.
//! - [`ear`] — ECCN parsing, CCL category keywords, license-exception
//!   and de-minimis extraction.
//! - [`jurisdiction`] — merges the ITAR + EAR reports and applies
//!   country / person rules to produce a final routing decision.
//! - [`tech_data`] — generic technical-data classifier (drawings,
//!   specs, design methodology) used to enrich ambiguous content.
//!
//! Trust Manifold projection (BF-2):
//!
//! - [`trust`] — `TrustContext` and supporting enums (service identity,
//!   compliance scope, allowed route, data classification, revocation
//!   status). Every Lamprey decision path from Session 39 onward
//!   accepts a `&TrustContext`. See `docs/TRUST-MANIFOLD.md` and
//!   `docs/SERVICE-IDENTITY.md` for the architecture.
//!
//! Tribal data sovereignty (Session 40):
//!
//! - [`ocap`] — OCAP (Ownership, Control, Access, Possession) policy
//!   engine. Sub-modules: tribal-identifier detection, treaty-aware
//!   routing, cultural-sensitivity filter, and the unified rules
//!   evaluator. Consumes `&TrustContext` on every decision.
//!
//! Wiring into the `mai-router` pipeline lands in Session 41 (policy
//! runtime); this crate is intentionally standalone so it can be reused
//! by audit reporting (Session 43) and the compliance dashboard
//! (Session 44).

#![forbid(unsafe_code)]

pub mod baa;
pub mod deid;
pub mod ear;
pub mod itar;
pub mod jurisdiction;
pub mod medical_entities;
pub mod ocap;
pub mod phi;
pub mod policy;
pub mod tech_data;
pub mod trust;
pub mod trust_cache;

pub use baa::{BaaConfig, BaaDecision, BaaEnforcer, BaaError, BaaMode, BaaViolation};
pub use deid::{DeidConfig, DeidResult, Redactor, RiskScore};
pub use ear::{
    CclCategory, CclKeywordHit, DeMinimisIndicator, EarClassification, EarDetector,
    EarDetectorConfig, EarError, EarReport, EccnHit, LicenseException, ProductGroup,
};
pub use itar::{
    ExportClassification, ItarConfidence, ItarDetector, ItarDetectorConfig, ItarError, ItarReport,
    UsmlCategory, UsmlHit,
};
pub use jurisdiction::{
    ActorContext, CountryCode, JurisdictionConfig, JurisdictionDecision, JurisdictionError,
    JurisdictionEvaluator, Outcome, PersonType, UnifiedClassification,
};
pub use medical_entities::{
    IcdValidator, LabValue, MedicationDictionary, MedicationHit, parse_lab_values,
};
pub use ocap::{
    AccessRole, ConsentStatus, CulturalConfidence, CulturalFilter, CulturalFilterConfig,
    CulturalHit, CulturalReport, CulturalSignal, GovernanceMetadata, OcapConfidence, OcapConfig,
    OcapDecision, OcapError, OcapEvaluator, OcapOutcome, OcapReason, PossessionStatus,
    TreatyDetector, TreatyDetectorConfig, TreatyHit, TreatyId, TreatyReport, TribalDataDetector,
    TribalDataDetectorConfig, TribalDataReport, TribalHit, TribalIdentifierKind,
};
pub use phi::{PhiConfidence, PhiDetector, PhiDetectorConfig, PhiHit, PhiIdentifier, PhiReport};
pub use policy::{ClassificationResult, PolicyBundle, PolicyBundleError, RequestMetadata};
pub use tech_data::{
    HeuristicTechDataClassifier, TechDataAssessment, TechDataClassifier, TechDataConfidence,
    TechDataError, TechDataHit, TechDataSignal,
};
pub use trust::{
    AllowedRoute, ComplianceScope, DataClassification, RevocationStatus, ServiceIdentity,
    SubjectHash, SubjectId, TenantId, TrustContext, TrustContextError,
};
