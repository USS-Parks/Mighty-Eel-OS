//! Compliance Report Generator.
//!
//! Produces regulator-ready compliance documentation from the
//! [`crate::audit::AuditLog`]. Each report is built by feeding a
//! [`ReportRequest`] (template + date range) into the [`engine::ReportEngine`],
//! which queries the audit log, runs the template, and emits a
//! [`ReportDocument`] in the caller's chosen [`ReportFormat`].
//!
//! Submodules:
//!
//! - [`engine`] — generation engine, format rendering, trust-section
//!   builder. The §A.13 trust gate (credential validation summary,
//!   trust bundle version, revocation snapshot, offline intervals,
//!   service identity events, policy version history, audit
//!   verification status) is enforced here: every report contains a
//!   `TrustSection` whether the template asks for it or not.
//! - [`templates`] — the five pre-built templates (HIPAA audit trail,
//!   ITAR/EAR compliance summary, OCAP governance, system activity,
//!   monthly digest) and the [`templates::ReportTemplate`] trait that
//!   third-party templates can implement.
//! - [`pdf`] — certification envelope. The "PDF" output is a text
//!   rendering with an embedded ML-DSA-87 signature over the BLAKE3
//!   of the canonical bytes. The dashboard can wrap this in a
//!   real PDF; the verifiable artefact is the
//!   [`pdf::CertifiedReport`].
//! - [`prune`] — retention-based pruning. Reports older than the
//!   per-type retention horizon are deleted unless marked protected.
//! - [`api`] — [`api::ReportManager`] façade: generate / list / get /
//!   delete, plus scheduling hooks.
//!
//! Report generation itself is audited: every successful generation
//! emits a `report.generated` policy-change event to the
//! [`crate::audit::AuditLog`] via [`api::ReportManager::record_generation_event`].
//!
//! HTTP wiring (`/v1/compliance/reports/*`) lives in `mai-api`; this
//! crate exposes the typed surface only.

pub mod api;
pub mod engine;
pub mod pdf;
pub mod prune;
pub mod templates;

pub use api::{
    ReportError, ReportId, ReportManager, ReportManagerBuilder, ReportRecord, ReportRequest,
    ReportSchedule, ReportScheduleId, ReportStatus,
};
pub use engine::{
    CredentialValidationSummary, OfflineInterval, PolicyVersionEvent, ReportDocument, ReportEngine,
    ReportFormat, ReportMetadata, ReportType, RevocationSnapshotSummary, ServiceIdentityEvent,
    TrustSection,
};
pub use pdf::{CertifiedReport, ReportCertifier, ReportSigner};
pub use prune::{PruneConfig, PruneOutcome, ReportPruner};
pub use templates::{
    HipaaAuditTrail, ItarComplianceSummary, MonthlyComplianceDigest, OcapGovernanceReport,
    ReportTemplate, SystemActivitySummary, TemplateRegistry,
};
