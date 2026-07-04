//! Report generation engine.
//!
//! [`ReportEngine`] turns a [`ReportType`] + date range into a
//! [`ReportDocument`] by:
//!
//! 1. Querying the [`crate::audit::AuditLog`] for the relevant entries.
//! 2. Asking the [`super::templates::ReportTemplate`] to compute its
//!    domain-specific summary.
//! 3. Building the §A.13 [`TrustSection`] (credential validation
//!    summary, trust bundle version, revocation snapshot status,
//!    offline / degraded intervals, service identity events, policy
//!    version history, audit verification status).
//! 4. Rendering the combined [`ReportPayload`] into the caller's
//!    requested [`ReportFormat`] (JSON / HTML / CSV / Text).
//!
//! The engine is pure: same audit log + template + clock → same
//! output. Persistence and scheduling are the [`super::api`] layer's
//! job.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::audit::{AuditLog, AuditQuery, AuditQueryRow, RoutingDecision, VerificationStatus};
use crate::policy::composer::ModuleId;

use super::templates::ReportTemplate;

/// Output formats supported by [`ReportEngine::render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    /// Machine-readable JSON. Exposes the full [`ReportPayload`]
    /// shape verbatim and is the canonical artefact signed by
    /// [`super::pdf::ReportCertifier`].
    Json,
    /// Human-readable HTML. Safe to embed in a dashboard.
    Html,
    /// Comma-separated row dump of the matched audit entries.
    Csv,
    /// Plain text rendering. Used as the body of the "PDF" output —
    /// the dashboard wraps this in real PDF chrome.
    Text,
}

impl ReportFormat {
    /// Wire-format identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Csv => "csv",
            Self::Text => "text",
        }
    }

    /// MIME content type emitted alongside the rendered bytes.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Html => "text/html; charset=utf-8",
            Self::Csv => "text/csv; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
        }
    }
}

/// Stable identifier for the five pre-built report templates plus an
/// escape hatch for downstream extensions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportType {
    /// HIPAA Audit Trail: every PHI access, policy decision, violation.
    HipaaAuditTrail,
    /// ITAR / EAR Compliance Summary: export-controlled queries.
    ItarComplianceSummary,
    /// OCAP Governance Report: tribal data access, treaty references.
    OcapGovernance,
    /// System Activity Summary: routing stats, module health.
    SystemActivity,
    /// Monthly Compliance Digest: executive cross-domain summary.
    MonthlyDigest,
    /// Caller-defined template id. Resolved via the
    /// [`super::templates::TemplateRegistry`].
    Custom(String),
}

impl ReportType {
    /// Wire-format identifier.
    pub fn as_str(&self) -> &str {
        match self {
            Self::HipaaAuditTrail => "hipaa_audit_trail",
            Self::ItarComplianceSummary => "itar_compliance_summary",
            Self::OcapGovernance => "ocap_governance",
            Self::SystemActivity => "system_activity",
            Self::MonthlyDigest => "monthly_digest",
            Self::Custom(s) => s.as_str(),
        }
    }
}

/// Provenance metadata recorded on every generated report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMetadata {
    /// Report type (stable identifier).
    pub report_type: ReportType,
    /// Inclusive lower bound on audit `timestamp_unix_nanos`.
    pub from_unix_nanos: u64,
    /// Inclusive upper bound on audit `timestamp_unix_nanos`.
    pub to_unix_nanos: u64,
    /// Wall-clock nanoseconds when the report was generated.
    pub generated_at_unix_nanos: u64,
    /// Active policy bundle version at generation time.
    pub policy_version: String,
    /// Number of audit entries that fell inside the date range
    /// (before tenant filtering).
    pub entries_considered: u64,
    /// Optional tenant filter applied to the query.
    pub tenant: Option<String>,
}

/// One credential-validation roll-up entry. Built by counting the
/// per-decision `correlation.credential_event_id` field across the
/// query window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialValidationSummary {
    /// Total decisions backed by a cloud credential event.
    pub credential_backed: u64,
    /// Total decisions with no `credential_event_id` (local-only /
    /// offline). Distinct from `failed_validations` — the request
    /// proceeded; it just did not consult the cloud trust core.
    pub local_only: u64,
    /// Decisions that surfaced a `trust.revoked` or
    /// `trust.revocation_unknown_*` rule match. These are *not*
    /// validation failures in the cryptographic sense (we never saw
    /// a bad signature here), but they are the cases an auditor will
    /// scan first.
    pub revocation_flagged: u64,
}

/// Snapshot of the revocation status mix recorded across the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationSnapshotSummary {
    /// Decisions where the recorded revocation status was `valid`.
    pub valid: u64,
    /// Decisions where the recorded revocation status was `revoked`.
    pub revoked: u64,
    /// Decisions where the recorded revocation status was `unknown`.
    pub unknown: u64,
}

/// One row in the offline / degraded interval table.
///
/// The audit log doesn't carry a `mode` field directly — we infer
/// degraded intervals from the absence of `credential_event_id` on
/// adjacent entries. The first entry with no credential event opens
/// an interval; the first credential-backed entry that follows
/// closes it. This is an approximation — the dashboard refines
/// it with the live `ConnectivityState` feed — but it's sufficient
/// for regulator-ready reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineInterval {
    /// Inclusive lower bound.
    pub from_unix_nanos: u64,
    /// Inclusive upper bound.
    pub to_unix_nanos: u64,
    /// Audit entries that landed in this interval.
    pub entry_count: u64,
}

/// One row in the service-identity event table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceIdentityEvent {
    /// Service identity (e.g. `"lamprey-router"`). `None` when an
    /// entry was recorded without one.
    pub service_identity: Option<String>,
    /// Total decisions recorded under this identity.
    pub decision_count: u64,
    /// First sighting in the window.
    pub first_seen_unix_nanos: u64,
    /// Last sighting in the window.
    pub last_seen_unix_nanos: u64,
}

/// One row in the policy-version history table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyVersionEvent {
    /// Policy bundle version string.
    pub policy_version: String,
    /// Earliest decision recorded under this version in the window.
    pub first_used_unix_nanos: u64,
    /// Latest decision recorded under this version in the window.
    pub last_used_unix_nanos: u64,
    /// Number of decisions made under this version.
    pub decision_count: u64,
}

/// Trust + credential section — the §A.13 gate.
///
/// Every report carries one of these regardless of template, so the
/// acquisition narrative can point at any generated report and
/// say "trust section appears here, here, and here."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustSection {
    /// Credential validation roll-up.
    pub credential_validation: CredentialValidationSummary,
    /// Unique trust bundle versions referenced in the window, in
    /// first-seen order.
    pub trust_bundle_versions: Vec<String>,
    /// Revocation status mix across the window.
    pub revocation_snapshot: RevocationSnapshotSummary,
    /// Approximated offline / degraded intervals (see
    /// [`OfflineInterval`] for the heuristic).
    pub offline_intervals: Vec<OfflineInterval>,
    /// Service identities observed in the window.
    pub service_identity_events: Vec<ServiceIdentityEvent>,
    /// Policy versions observed in the window.
    pub policy_version_history: Vec<PolicyVersionEvent>,
    /// Audit chain verification status at report generation time.
    pub audit_verification: VerificationStatus,
}

/// Per-module decision count, surfaced into the system-activity
/// section and the digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleActivity {
    /// Module id.
    pub module: ModuleId,
    /// Decisions where this module appeared in `modules_applied`.
    pub decision_count: u64,
    /// Decisions where this module's verdict was `deny`.
    pub deny_count: u64,
    /// Decisions where this module forced a `local_only` route.
    pub local_only_count: u64,
    /// Decisions where this module forced a `quarantine` route.
    pub quarantine_count: u64,
}

/// Rolled-up decision counts across the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTotals {
    /// `allow` decisions.
    pub allow: u64,
    /// `local_only_allowed` decisions.
    pub local_only: u64,
    /// `quarantine` decisions.
    pub quarantine: u64,
    /// `deny` decisions.
    pub deny: u64,
}

impl DecisionTotals {
    /// Sum across all four outcomes.
    pub fn total(&self) -> u64 {
        self.allow + self.local_only + self.quarantine + self.deny
    }
}

/// The full payload the engine hands to the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportPayload {
    /// Provenance.
    pub metadata: ReportMetadata,
    /// Top-of-report summary text from the template.
    pub summary: String,
    /// Per-module activity.
    pub module_activity: Vec<ModuleActivity>,
    /// Aggregate decision totals.
    pub decision_totals: DecisionTotals,
    /// §A.13 trust + credential section.
    pub trust: TrustSection,
    /// Template-specific narrative sections, in stable order.
    pub sections: Vec<ReportSection>,
    /// Matched audit rows the template chose to surface, capped by
    /// [`ReportEngine::row_limit`]. Empty for digest-style reports.
    pub rows: Vec<AuditQueryRow>,
}

/// One template-defined narrative section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSection {
    /// Section heading.
    pub heading: String,
    /// Body text. Plain (no markup); the renderer escapes HTML
    /// where appropriate.
    pub body: String,
}

/// A rendered report ready to hand back to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDocument {
    /// Provenance metadata (also embedded in the payload).
    pub metadata: ReportMetadata,
    /// Format the bytes were rendered in.
    pub format: ReportFormat,
    /// Rendered bytes.
    pub body: Vec<u8>,
    /// Structured payload (always carried so callers can re-render
    /// or sign without re-running the engine).
    pub payload: ReportPayload,
}

impl ReportDocument {
    /// Body as UTF-8 string. JSON / HTML / CSV / Text are all valid
    /// UTF-8 by construction.
    pub fn body_as_str(&self) -> &str {
        std::str::from_utf8(&self.body).expect("report renderer emits valid UTF-8")
    }
}

/// Errors raised by the engine.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// Caller supplied an inverted date range.
    #[error("invalid date range: from ({from}) > to ({to})")]
    InvalidRange {
        /// Lower bound.
        from: u64,
        /// Upper bound.
        to: u64,
    },
    /// Caller asked for a [`ReportType::Custom`] that no template
    /// implements.
    #[error("no template registered for report type {0:?}")]
    UnknownTemplate(ReportType),
    /// JSON rendering failed (should be infallible — surfaced for
    /// completeness).
    #[error("JSON rendering failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// The report generation engine.
///
/// Holds a clone of the [`AuditLog`] so the same engine instance
/// can be reused across multiple report generations.
#[derive(Debug, Clone)]
pub struct ReportEngine {
    audit: AuditLog,
    row_limit: usize,
}

impl ReportEngine {
    /// Construct an engine with the default row limit (1000).
    pub fn new(audit: AuditLog) -> Self {
        Self {
            audit,
            row_limit: 1000,
        }
    }

    /// Override the per-report row cap.
    pub fn with_row_limit(mut self, limit: usize) -> Self {
        self.row_limit = limit;
        self
    }

    /// Maximum number of audit rows any single report will embed.
    pub fn row_limit(&self) -> usize {
        self.row_limit
    }

    /// Build the payload for a report request and render it.
    pub fn generate(
        &self,
        template: &dyn ReportTemplate,
        request: &super::api::ReportRequest,
        format: ReportFormat,
        policy_version: &str,
        now_unix_nanos: u64,
    ) -> Result<ReportDocument, ReportError> {
        if request.from_unix_nanos > request.to_unix_nanos {
            return Err(ReportError::InvalidRange {
                from: request.from_unix_nanos,
                to: request.to_unix_nanos,
            });
        }
        let query = AuditQuery {
            from: Some(request.from_unix_nanos),
            to: Some(request.to_unix_nanos),
            module: template.scope_module(),
            decision: None,
            tenant: request.tenant.clone(),
            limit: None,
        };
        let rows = self.audit.query(&query);
        let entries_considered = rows.len() as u64;

        let module_activity = compute_module_activity(&rows);
        let decision_totals = compute_decision_totals(&rows);
        let trust = compute_trust_section(&rows, &self.audit);

        let metadata = ReportMetadata {
            report_type: template.report_type(),
            from_unix_nanos: request.from_unix_nanos,
            to_unix_nanos: request.to_unix_nanos,
            generated_at_unix_nanos: now_unix_nanos,
            policy_version: policy_version.to_string(),
            entries_considered,
            tenant: request.tenant.clone(),
        };

        let template_ctx = TemplateContext {
            metadata: &metadata,
            module_activity: &module_activity,
            decision_totals: &decision_totals,
            trust: &trust,
            rows: &rows,
        };
        let (summary, sections) = template.build(template_ctx);

        let mut payload_rows = rows;
        if payload_rows.len() > self.row_limit {
            payload_rows.truncate(self.row_limit);
        }

        let payload = ReportPayload {
            metadata: metadata.clone(),
            summary,
            module_activity,
            decision_totals,
            trust,
            sections,
            rows: payload_rows,
        };

        let body = render_payload(&payload, format)?;
        Ok(ReportDocument {
            metadata,
            format,
            body,
            payload,
        })
    }
}

/// Context handed to [`ReportTemplate::build`]. Borrowed so templates
/// don't allocate just to read.
#[derive(Debug, Clone, Copy)]
pub struct TemplateContext<'a> {
    /// Provenance.
    pub metadata: &'a ReportMetadata,
    /// Per-module activity, sorted by module id.
    pub module_activity: &'a [ModuleActivity],
    /// Aggregate decision totals.
    pub decision_totals: &'a DecisionTotals,
    /// §A.13 trust section.
    pub trust: &'a TrustSection,
    /// All audit rows that matched the query (uncapped — templates
    /// receive the full slice so they can compute their own
    /// statistics).
    pub rows: &'a [AuditQueryRow],
}

fn compute_module_activity(rows: &[AuditQueryRow]) -> Vec<ModuleActivity> {
    let mut by_module: BTreeMap<ModuleId, ModuleActivity> = BTreeMap::new();
    for row in rows {
        for m in &row.entry.modules_applied {
            let slot = by_module.entry(*m).or_insert(ModuleActivity {
                module: *m,
                decision_count: 0,
                deny_count: 0,
                local_only_count: 0,
                quarantine_count: 0,
            });
            slot.decision_count += 1;
            match row.entry.decision {
                RoutingDecision::Deny => slot.deny_count += 1,
                RoutingDecision::LocalOnly => slot.local_only_count += 1,
                RoutingDecision::Quarantine => slot.quarantine_count += 1,
                RoutingDecision::Allow => {}
            }
        }
    }
    by_module.into_values().collect()
}

fn compute_decision_totals(rows: &[AuditQueryRow]) -> DecisionTotals {
    let mut totals = DecisionTotals::default();
    for row in rows {
        match row.entry.decision {
            RoutingDecision::Allow => totals.allow += 1,
            RoutingDecision::LocalOnly => totals.local_only += 1,
            RoutingDecision::Quarantine => totals.quarantine += 1,
            RoutingDecision::Deny => totals.deny += 1,
        }
    }
    totals
}

fn compute_trust_section(rows: &[AuditQueryRow], audit: &AuditLog) -> TrustSection {
    let mut credential_backed = 0u64;
    let mut local_only = 0u64;
    let mut revocation_flagged = 0u64;
    let mut bundle_versions_seen: Vec<String> = Vec::new();
    let mut bundle_versions_set: BTreeSet<String> = BTreeSet::new();
    let mut snapshot = RevocationSnapshotSummary {
        valid: 0,
        revoked: 0,
        unknown: 0,
    };
    let mut svc_events: BTreeMap<Option<String>, ServiceIdentityEvent> = BTreeMap::new();
    let mut policy_events: BTreeMap<String, PolicyVersionEvent> = BTreeMap::new();
    let mut offline_intervals: Vec<OfflineInterval> = Vec::new();
    let mut current_offline: Option<OfflineInterval> = None;

    for row in rows {
        let c = &row.entry.correlation;
        if c.credential_event_id.is_some() {
            credential_backed += 1;
            if let Some(open) = current_offline.take() {
                offline_intervals.push(open);
            }
        } else {
            local_only += 1;
            let ts = row.entry.timestamp_unix_nanos;
            match current_offline.as_mut() {
                Some(window) => {
                    window.to_unix_nanos = ts;
                    window.entry_count += 1;
                }
                None => {
                    current_offline = Some(OfflineInterval {
                        from_unix_nanos: ts,
                        to_unix_nanos: ts,
                        entry_count: 1,
                    });
                }
            }
        }
        for rm in &row.entry.rules_fired {
            if let Some(rule) = rm.rule.as_deref()
                && (rule.starts_with("trust.revoked")
                    || rule.starts_with("trust.revocation_unknown"))
            {
                revocation_flagged += 1;
            }
        }
        if bundle_versions_set.insert(c.trust_bundle_version.clone()) {
            bundle_versions_seen.push(c.trust_bundle_version.clone());
        }
        // Snapshot status mix is inferred from the trust rules
        // (the audit entry doesn't carry a raw revocation enum).
        let revocation_rule = row
            .entry
            .rules_fired
            .iter()
            .filter_map(|rm| rm.rule.as_deref())
            .find(|r| r.starts_with("trust."));
        match revocation_rule {
            Some(r) if r.contains("revoked") => snapshot.revoked += 1,
            Some(r) if r.contains("unknown") => snapshot.unknown += 1,
            _ => snapshot.valid += 1,
        }

        let svc_key = c.service_identity.clone();
        let svc_ts = row.entry.timestamp_unix_nanos;
        svc_events
            .entry(svc_key.clone())
            .and_modify(|e| {
                e.decision_count += 1;
                if svc_ts < e.first_seen_unix_nanos {
                    e.first_seen_unix_nanos = svc_ts;
                }
                if svc_ts > e.last_seen_unix_nanos {
                    e.last_seen_unix_nanos = svc_ts;
                }
            })
            .or_insert(ServiceIdentityEvent {
                service_identity: svc_key,
                decision_count: 1,
                first_seen_unix_nanos: svc_ts,
                last_seen_unix_nanos: svc_ts,
            });

        let pv = c.policy_version.clone();
        let ts = row.entry.timestamp_unix_nanos;
        policy_events
            .entry(pv.clone())
            .and_modify(|e| {
                e.decision_count += 1;
                if ts < e.first_used_unix_nanos {
                    e.first_used_unix_nanos = ts;
                }
                if ts > e.last_used_unix_nanos {
                    e.last_used_unix_nanos = ts;
                }
            })
            .or_insert(PolicyVersionEvent {
                policy_version: pv,
                first_used_unix_nanos: ts,
                last_used_unix_nanos: ts,
                decision_count: 1,
            });
    }
    if let Some(open) = current_offline.take() {
        offline_intervals.push(open);
    }

    let audit_verification = audit.integrity_status().last_verify;

    TrustSection {
        credential_validation: CredentialValidationSummary {
            credential_backed,
            local_only,
            revocation_flagged,
        },
        trust_bundle_versions: bundle_versions_seen,
        revocation_snapshot: snapshot,
        offline_intervals,
        service_identity_events: svc_events.into_values().collect(),
        policy_version_history: policy_events.into_values().collect(),
        audit_verification,
    }
}

fn render_payload(payload: &ReportPayload, format: ReportFormat) -> Result<Vec<u8>, ReportError> {
    match format {
        ReportFormat::Json => Ok(serde_json::to_vec_pretty(payload)?),
        ReportFormat::Html => Ok(render_html(payload).into_bytes()),
        ReportFormat::Csv => Ok(render_csv(payload).into_bytes()),
        ReportFormat::Text => Ok(render_text(payload).into_bytes()),
    }
}

fn render_text(payload: &ReportPayload) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "Island Mountain MAI — Lamprey Compliance Report");
    let _ = writeln!(s, "================================================");
    let _ = writeln!(
        s,
        "Report type   : {}",
        payload.metadata.report_type.as_str()
    );
    let _ = writeln!(
        s,
        "Date range    : {} → {}",
        payload.metadata.from_unix_nanos, payload.metadata.to_unix_nanos
    );
    let _ = writeln!(
        s,
        "Generated at  : {}",
        payload.metadata.generated_at_unix_nanos
    );
    let _ = writeln!(s, "Policy version: {}", payload.metadata.policy_version);
    if let Some(t) = &payload.metadata.tenant {
        let _ = writeln!(s, "Tenant filter : {t}");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "Summary");
    let _ = writeln!(s, "-------");
    let _ = writeln!(s, "{}", payload.summary);
    let _ = writeln!(s);
    let _ = writeln!(s, "Decision totals");
    let _ = writeln!(s, "---------------");
    let _ = writeln!(s, "  allow      : {}", payload.decision_totals.allow);
    let _ = writeln!(s, "  local_only : {}", payload.decision_totals.local_only);
    let _ = writeln!(s, "  quarantine : {}", payload.decision_totals.quarantine);
    let _ = writeln!(s, "  deny       : {}", payload.decision_totals.deny);
    let _ = writeln!(s);
    let _ = writeln!(s, "Trust + credential section");
    let _ = writeln!(s, "--------------------------");
    let _ = writeln!(
        s,
        "  credential-backed decisions : {}",
        payload.trust.credential_validation.credential_backed
    );
    let _ = writeln!(
        s,
        "  local-only decisions        : {}",
        payload.trust.credential_validation.local_only
    );
    let _ = writeln!(
        s,
        "  revocation-flagged          : {}",
        payload.trust.credential_validation.revocation_flagged
    );
    let _ = writeln!(
        s,
        "  trust bundle versions       : {}",
        payload.trust.trust_bundle_versions.join(", ")
    );
    let _ = writeln!(
        s,
        "  offline intervals           : {}",
        payload.trust.offline_intervals.len()
    );
    let _ = writeln!(
        s,
        "  audit verification          : {:?}",
        payload.trust.audit_verification
    );
    let _ = writeln!(s);
    for section in &payload.sections {
        let _ = writeln!(s, "{}", section.heading);
        let _ = writeln!(s, "{}", "-".repeat(section.heading.len()));
        let _ = writeln!(s, "{}", section.body);
        let _ = writeln!(s);
    }
    let _ = writeln!(
        s,
        "-- Generated by Island Mountain MAI — Lamprey Compliance Layer --"
    );
    s
}

fn render_html(payload: &ReportPayload) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body>",
        html_escape(payload.metadata.report_type.as_str())
    );
    let _ = writeln!(
        s,
        "<h1>Island Mountain MAI — {}</h1>",
        html_escape(payload.metadata.report_type.as_str())
    );
    let _ = writeln!(
        s,
        "<p><b>Date range:</b> {} → {}<br>\
         <b>Policy version:</b> {}<br>\
         <b>Generated at:</b> {}<br>\
         <b>Audit verification:</b> {:?}</p>",
        payload.metadata.from_unix_nanos,
        payload.metadata.to_unix_nanos,
        html_escape(&payload.metadata.policy_version),
        payload.metadata.generated_at_unix_nanos,
        payload.trust.audit_verification
    );
    let _ = writeln!(
        s,
        "<h2>Summary</h2><p>{}</p>",
        html_escape(&payload.summary)
    );
    let _ = writeln!(
        s,
        "<h2>Decision totals</h2><ul>\
         <li>allow: {}</li>\
         <li>local_only: {}</li>\
         <li>quarantine: {}</li>\
         <li>deny: {}</li></ul>",
        payload.decision_totals.allow,
        payload.decision_totals.local_only,
        payload.decision_totals.quarantine,
        payload.decision_totals.deny
    );
    let _ = writeln!(
        s,
        "<h2>Trust + credential section</h2><ul>\
         <li>credential-backed: {}</li>\
         <li>local-only: {}</li>\
         <li>revocation-flagged: {}</li>\
         <li>trust bundle versions: {}</li>\
         <li>offline intervals: {}</li></ul>",
        payload.trust.credential_validation.credential_backed,
        payload.trust.credential_validation.local_only,
        payload.trust.credential_validation.revocation_flagged,
        html_escape(&payload.trust.trust_bundle_versions.join(", ")),
        payload.trust.offline_intervals.len()
    );
    for section in &payload.sections {
        let _ = writeln!(
            s,
            "<h2>{}</h2><pre>{}</pre>",
            html_escape(&section.heading),
            html_escape(&section.body)
        );
    }
    let _ = writeln!(
        s,
        "<footer><small>Generated by Island Mountain MAI — Lamprey Compliance Layer</small></footer></body></html>"
    );
    s
}

fn render_csv(payload: &ReportPayload) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "id,timestamp_unix_nanos,decision,modules_applied,routing_reason,tenant,trust_bundle_version,credential_event_id"
    );
    for row in &payload.rows {
        let e = &row.entry;
        let modules: Vec<&str> = e.modules_applied.iter().map(|m| m.as_str()).collect();
        let _ = writeln!(
            s,
            "{},{},{},{},{},{},{},{}",
            e.id,
            e.timestamp_unix_nanos,
            e.decision.as_str(),
            csv_field(&modules.join("|")),
            csv_field(&e.routing_reason),
            csv_field(&e.correlation.tenant),
            csv_field(&e.correlation.trust_bundle_version),
            csv_field(e.correlation.credential_event_id.as_deref().unwrap_or(""))
        );
    }
    s
}

fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
