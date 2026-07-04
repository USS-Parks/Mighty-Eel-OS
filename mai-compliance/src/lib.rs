//! MAI Compliance Engine.
//!
//! HIPAA-specific modules:
//!
//! - [`phi`] — detector for all 18 HIPAA Safe Harbor identifiers with
//!   Explicit / Probable / Possible confidence levels.
//! - [`baa`] — Business Associate Agreement enforcement engine with
//!   Standard / Strict / Custom modes.
//! - [`deid`] — PHI redaction with re-identification risk scoring.
//! - [`medical_entities`] — ICD-10 code validation, RxNorm medication
//!   dictionary, and lab-value parsing that enrich routing decisions.
//!
//! Export-control modules:
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
//! Trust Manifold projection:
//!
//! - [`trust`] — `TrustContext` and supporting enums (service identity,
//!   compliance scope, allowed route, data classification, revocation
//!   status). Every Lamprey decision path
//!   accepts a `&TrustContext`. See `docs/TRUST-MANIFOLD.md` and
//!   `docs/SERVICE-IDENTITY.md` for the architecture.
//!
//! Tribal data sovereignty:
//!
//! - [`ocap`] — OCAP (Ownership, Control, Access, Possession) policy
//!   engine. Sub-modules: tribal-identifier detection, treaty-aware
//!   routing, cultural-sensitivity filter, and the unified rules
//!   evaluator. Consumes `&TrustContext` on every decision.
//!
//! Trust bundle verification:
//!
//! - [`bundle`] — ML-DSA-87-backed verifier for signed policy bundles
//!   and signed claims. See `docs/TRUST-BUNDLE-SPEC.md` for the wire
//!   format and verification algorithm.
//! - [`subject_hash`] — HMAC-SHA256 pseudonymization of subject ids for
//!   audit correlation.
//!
//! Wiring into the `mai-router` pipeline (policy
//! runtime); this crate is intentionally standalone so it can be reused
//! by audit reporting and the compliance dashboard
//!
//! Compliance report generator:
//!
//! - [`reports`] — [`reports::ReportManager`] turns audit-log entries
//!   into regulator-ready documents (HIPAA audit trail, ITAR/EAR
//!   summary, OCAP governance, system activity, monthly digest), with
//!   the §A.13 trust + credential section attached to every report.
//!   The certification envelope is signed with the same ML-DSA-87
//!   primitive used by [`bundle`] and [`audit`].

#![forbid(unsafe_code)]

pub mod audit;
pub mod baa;
pub mod bundle;
pub mod deid;
pub mod ear;
pub mod itar;
pub mod jurisdiction;
pub mod medical_entities;
pub mod ocap;
pub mod phi;
pub mod policy;
pub mod reports;
pub mod subject_hash;
pub mod tech_data;
pub mod trust;
pub mod trust_cache;

pub use audit::{
    AEAD_SEALER_KEY_LEN, AEAD_SEALER_NONCE_LEN, AeadSealer, AeadSealerError, AuditEntry, AuditLog,
    AuditLogBuilder, AuditQuery, AuditQueryRow, AuditRecordInput, AuditStore, AuditStoreConfig,
    CHAIN_HASH_LEN, ChainConfig, ChainError, ChainSigner, CorrelationFields,
    DEFAULT_RETENTION_DAYS, DEFAULT_SIGNATURE_INTERVAL, EntriesById, Escalation, HashChainManager,
    IntegrityStatus, MlDsaChainSigner, NullSealer, NullSigner, RoutingDecision, RuleMatch,
    SIGNATURE_LEN, Severity, StoreDropCounters, StoreError, StoreSealer, TriggerManager,
    TriggersConfig, VerificationStatus, masked_request_hash, verify_chain,
};
pub use baa::{BaaConfig, BaaDecision, BaaEnforcer, BaaError, BaaMode, BaaViolation};
pub use bundle::{
    AcceptAllBundleVerifier, BundleError, BundleMetadata, BundleVerifier, ClaimPayload,
    MlDsaBundleVerifier, PolicyBundlePayload, RejectAllBundleVerifier, SignatureEnvelope,
    SignedClaim, SignedPolicyBundle,
};
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
pub use policy::{
    AggregateDecision, AuditFeed, ClassificationResult, ComplianceFlag, ComplianceReason,
    ComposerConfig, DecisionCache, DecisionCacheConfig, DecisionKey, Destination, FeedEvent,
    FeedSubscriber, ModuleDecision, ModuleId, ModuleStatus, OverallStatus, PolicyBundle,
    PolicyBundleError, PolicyComposer, PolicyManager, PolicySource, PolicyTemplate,
    RequestMetadata, TemplateVersion,
};
pub use reports::{
    CertifiedReport, CredentialValidationSummary, HipaaAuditTrail, ItarComplianceSummary,
    MonthlyComplianceDigest, OcapGovernanceReport, OfflineInterval, PolicyVersionEvent,
    PruneConfig, PruneOutcome, ReportCertifier, ReportDocument, ReportEngine, ReportError,
    ReportFormat, ReportId, ReportManager, ReportManagerBuilder, ReportMetadata, ReportPruner,
    ReportRecord, ReportRequest, ReportSchedule, ReportScheduleId, ReportSigner, ReportStatus,
    ReportTemplate, ReportType, RevocationSnapshotSummary, ServiceIdentityEvent,
    SystemActivitySummary, TemplateRegistry, TrustSection,
};
pub use subject_hash::{SubjectHashError, hmac_subject};
pub use tech_data::{
    HeuristicTechDataClassifier, TechDataAssessment, TechDataClassifier, TechDataConfidence,
    TechDataError, TechDataHit, TechDataSignal,
};
pub use trust::{
    AllowedRoute, ComplianceScope, DataClassification, RevocationStatus, ServiceIdentity,
    SubjectHash, SubjectId, TenantId, TrustContext, TrustContextError,
};
