//! MAI HIPAA Compliance Engine (Session 38).
//!
//! Four modules compose the HIPAA-specific compliance stack that layers on
//! top of the Session 37 policy framework:
//!
//! - [`phi`] — detector for all 18 HIPAA Safe Harbor identifiers with
//!   Explicit / Probable / Possible confidence levels.
//! - [`baa`] — Business Associate Agreement enforcement engine with
//!   Standard / Strict / Custom modes.
//! - [`deid`] — PHI redaction with re-identification risk scoring.
//! - [`medical_entities`] — ICD-10 code validation, RxNorm medication
//!   dictionary, and lab-value parsing that enrich routing decisions.
//!
//! Wiring into the `mai-router` pipeline lands in Session 41 (policy
//! runtime); this crate is intentionally standalone so it can be reused
//! by audit reporting (Session 43) and the compliance dashboard
//! (Session 44).

#![forbid(unsafe_code)]

pub mod baa;
pub mod deid;
pub mod medical_entities;
pub mod phi;

pub use baa::{BaaConfig, BaaDecision, BaaEnforcer, BaaError, BaaMode, BaaViolation};
pub use deid::{DeidConfig, DeidResult, Redactor, RiskScore};
pub use medical_entities::{
    IcdValidator, LabValue, MedicationDictionary, MedicationHit, parse_lab_values,
};
pub use phi::{PhiConfidence, PhiDetector, PhiDetectorConfig, PhiHit, PhiIdentifier, PhiReport};
