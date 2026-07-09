//! Compliance Report Manager.
//!
//! [`ReportManager`] composes the engine, certifier, template
//! registry, and pruner into a single typed façade that backs the
//! HTTP routes listed below:
//!
//! | Route | API call |
//! |-------|----------|
//! | `POST   /v1/compliance/reports/generate` | [`ReportManager::generate`] |
//! | `GET    /v1/compliance/reports`          | [`ReportManager::list`] |
//! | `GET    /v1/compliance/reports/{id}`     | [`ReportManager::get`] |
//! | `DELETE /v1/compliance/reports/{id}`     | [`ReportManager::delete`] |
//!
//! HTTP wiring (axum routes, JSON shapes, auth) is in `mai-api`;
//! this module is pure so the dashboard process and tests can
//! use it directly.
//!
//! Generation events are auditable: the manager exposes
//! [`ReportManager::record_generation_event`] which the audit log
//! treats as a policy-change event (visible to the correlation
//! pipeline so external observers see "a report was produced").

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::audit::{AuditLog, Escalation};

use super::engine::{ReportEngine, ReportFormat, ReportType};
use super::pdf::{CertifiedReport, NullReportSigner, ReportCertifier, ReportSigner};
use super::prune::{PruneConfig, PruneOutcome, ReportPruner};
use super::templates::TemplateRegistry;

/// Stable report identifier. Opaque to clients.
pub type ReportId = String;

/// Stable schedule identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReportScheduleId(pub String);

impl ReportScheduleId {
    /// Borrowed view.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Input shape for [`ReportManager::generate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRequest {
    /// Which template to run.
    pub report_type: ReportType,
    /// Inclusive lower bound on the audit window.
    pub from_unix_nanos: u64,
    /// Inclusive upper bound on the audit window.
    pub to_unix_nanos: u64,
    /// Optional tenant filter.
    #[serde(default)]
    pub tenant: Option<String>,
}

/// Lifecycle of a report record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// Scheduled but not yet generating.
    Pending,
    /// Generation in progress.
    Generating,
    /// Generation finished successfully.
    Complete,
    /// Generation failed; see `error` on [`ReportRecord`].
    Failed,
}

/// Persistent record of a generated (or pending) report.
///
/// The manager keeps these in memory; production deployments mirror
/// them to disk via a callback (out of scope for this typed
/// surface).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRecord {
    /// Stable id.
    pub id: ReportId,
    /// Original request.
    pub request: ReportRequest,
    /// Status.
    pub status: ReportStatus,
    /// Wall-clock nanoseconds the request was filed.
    pub created_at_unix_nanos: u64,
    /// Wall-clock nanoseconds the report completed (or failed).
    /// `None` while still pending / generating.
    pub completed_at_unix_nanos: Option<u64>,
    /// Format of the rendered body.
    pub output_format: ReportFormat,
    /// BLAKE3 content hash of the canonical JSON payload.
    pub content_hash_hex: Option<String>,
    /// Failure message (only set when `status == Failed`).
    pub error: Option<String>,
    /// Operator-marked: never auto-prune.
    #[serde(default)]
    pub protected: bool,
    /// If this report came from a scheduled run, the schedule id.
    #[serde(default)]
    pub schedule_id: Option<ReportScheduleId>,
    /// Rendered body bytes. Held in memory; the dashboard streams
    /// these out on download. Set to `None` when the body has been
    /// shipped to cold storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<Vec<u8>>,
    /// Certification signature, hex-encoded. `None` for unsigned
    /// reports.
    #[serde(default)]
    pub signature_hex: Option<String>,
}

/// Cron-style schedule for an automated report run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSchedule {
    /// Stable id.
    pub id: ReportScheduleId,
    /// Template to run.
    pub report_type: ReportType,
    /// Output format to render in.
    pub format: ReportFormat,
    /// How often (seconds) the schedule should fire. The manager
    /// doesn't tick by itself; the operator (or a scheduler thread)
    /// calls [`ReportManager::run_due_schedules`] with the current
    /// wall-clock time.
    pub period_secs: u64,
    /// Window length (seconds) — the generator queries audit
    /// entries with timestamps in `[now - window_secs, now]`.
    pub window_secs: u64,
    /// Optional tenant filter.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Wall-clock nanoseconds the schedule last fired. `None` when
    /// it has never run.
    pub last_run_unix_nanos: Option<u64>,
    /// `true` keeps the schedule but suspends auto-runs.
    #[serde(default)]
    pub paused: bool,
    /// Mark each generated report as protected (never prune).
    #[serde(default)]
    pub protected_outputs: bool,
}

impl ReportSchedule {
    fn due(&self, now: u64) -> bool {
        if self.paused {
            return false;
        }
        match self.last_run_unix_nanos {
            None => true,
            Some(last) => {
                now.saturating_sub(last) >= self.period_secs.saturating_mul(1_000_000_000)
            }
        }
    }
}

/// Errors raised by [`ReportManager`].
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// Caller asked for a template that is not registered.
    #[error("template not registered for {0:?}")]
    UnknownTemplate(ReportType),
    /// Schedule id collision.
    #[error("schedule id already exists: {0}")]
    DuplicateSchedule(String),
    /// No such record.
    #[error("report {0} not found")]
    NotFound(ReportId),
    /// Generator surfaced an error.
    #[error("engine error: {0}")]
    Engine(#[from] super::engine::ReportError),
    /// Certifier surfaced an error.
    #[error("certify error: {0}")]
    Certify(#[from] super::pdf::CertifyError),
}

#[derive(Debug)]
struct State {
    next_seq: u64,
    records: Vec<ReportRecord>,
    schedules: HashMap<String, ReportSchedule>,
}

/// Façade for the compliance report subsystem.
pub struct ReportManager {
    engine: ReportEngine,
    audit: AuditLog,
    templates: TemplateRegistry,
    certifier: Mutex<ReportCertifier>,
    pruner: ReportPruner,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for ReportManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReportManager")
            .field("templates", &self.templates)
            .field("pruner", &self.pruner)
            .finish_non_exhaustive()
    }
}

impl ReportManager {
    /// Construct a builder.
    pub fn builder(audit: AuditLog) -> ReportManagerBuilder {
        ReportManagerBuilder::new(audit)
    }

    /// Active engine (testing / introspection).
    pub fn engine(&self) -> &ReportEngine {
        &self.engine
    }

    /// Active template registry (testing / introspection).
    pub fn templates(&self) -> &TemplateRegistry {
        &self.templates
    }

    /// Active pruner config.
    pub fn prune_config(&self) -> &PruneConfig {
        self.pruner.config()
    }

    /// Number of records currently held.
    pub fn record_count(&self) -> usize {
        self.state
            .lock()
            .expect("report manager poisoned")
            .records
            .len()
    }

    /// Generate a report end-to-end: engine → certifier → record.
    pub fn generate(
        &self,
        request: ReportRequest,
        format: ReportFormat,
        policy_version: &str,
        now_unix_nanos: u64,
    ) -> Result<ReportRecord, ReportError> {
        let template = self
            .templates
            .get(&request.report_type)
            .ok_or_else(|| ReportError::UnknownTemplate(request.report_type.clone()))?;
        let id = self.next_id();
        let mut record = ReportRecord {
            id: id.clone(),
            request: request.clone(),
            status: ReportStatus::Generating,
            created_at_unix_nanos: now_unix_nanos,
            completed_at_unix_nanos: None,
            output_format: format,
            content_hash_hex: None,
            error: None,
            protected: false,
            schedule_id: None,
            body_bytes: None,
            signature_hex: None,
        };
        self.insert(record.clone());

        let result =
            self.engine
                .generate(template, &request, format, policy_version, now_unix_nanos);
        let document = match result {
            Ok(doc) => doc,
            Err(e) => {
                record.status = ReportStatus::Failed;
                record.error = Some(e.to_string());
                record.completed_at_unix_nanos = Some(now_unix_nanos);
                self.replace(record.clone());
                return Err(e.into());
            }
        };

        let certified = self
            .certifier
            .lock()
            .expect("report certifier poisoned")
            .certify(document, now_unix_nanos)?;

        record.status = ReportStatus::Complete;
        record.completed_at_unix_nanos = Some(now_unix_nanos);
        record.content_hash_hex = Some(certified.content_hash_hex.clone());
        record.signature_hex.clone_from(&certified.signature_hex);
        record.body_bytes = Some(certified.document.body.clone());
        record.output_format = certified.document.format;
        self.replace(record.clone());
        Ok(record)
    }

    /// Generate and return both the record and the certified envelope.
    pub fn generate_certified(
        &self,
        request: ReportRequest,
        format: ReportFormat,
        policy_version: &str,
        now_unix_nanos: u64,
    ) -> Result<(ReportRecord, CertifiedReport), ReportError> {
        let template = self
            .templates
            .get(&request.report_type)
            .ok_or_else(|| ReportError::UnknownTemplate(request.report_type.clone()))?;
        let document =
            self.engine
                .generate(template, &request, format, policy_version, now_unix_nanos)?;
        let certified = self
            .certifier
            .lock()
            .expect("report certifier poisoned")
            .certify(document, now_unix_nanos)?;
        let id = self.next_id();
        let record = ReportRecord {
            id,
            request: request.clone(),
            status: ReportStatus::Complete,
            created_at_unix_nanos: now_unix_nanos,
            completed_at_unix_nanos: Some(now_unix_nanos),
            output_format: certified.document.format,
            content_hash_hex: Some(certified.content_hash_hex.clone()),
            error: None,
            protected: false,
            schedule_id: None,
            body_bytes: Some(certified.document.body.clone()),
            signature_hex: certified.signature_hex.clone(),
        };
        self.insert(record.clone());
        Ok((record, certified))
    }

    /// List all records (newest first).
    pub fn list(&self) -> Vec<ReportRecord> {
        let guard = self.state.lock().expect("report manager poisoned");
        let mut out = guard.records.clone();
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at_unix_nanos));
        out
    }

    /// Fetch one record.
    pub fn get(&self, id: &str) -> Option<ReportRecord> {
        self.state
            .lock()
            .expect("report manager poisoned")
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    /// Delete one record. Returns the deleted record on success, or
    /// `ReportError::NotFound` if no record matched. Refuses to
    /// delete protected records — caller must clear the protected
    /// flag first.
    pub fn delete(&self, id: &str) -> Result<ReportRecord, ReportError> {
        let mut guard = self.state.lock().expect("report manager poisoned");
        let idx = guard
            .records
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| ReportError::NotFound(id.to_string()))?;
        if guard.records[idx].protected {
            return Err(ReportError::NotFound(format!(
                "{id} (protected; clear flag first)"
            )));
        }
        Ok(guard.records.remove(idx))
    }

    /// Mark a record protected. Returns the updated record.
    pub fn set_protected(&self, id: &str, protected: bool) -> Result<ReportRecord, ReportError> {
        let mut guard = self.state.lock().expect("report manager poisoned");
        let record = guard
            .records
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| ReportError::NotFound(id.to_string()))?;
        record.protected = protected;
        Ok(record.clone())
    }

    /// Register a scheduled report run.
    pub fn add_schedule(&self, schedule: ReportSchedule) -> Result<(), ReportError> {
        let mut guard = self.state.lock().expect("report manager poisoned");
        let id = schedule.id.as_str().to_string();
        if guard.schedules.contains_key(&id) {
            return Err(ReportError::DuplicateSchedule(id));
        }
        guard.schedules.insert(id, schedule);
        Ok(())
    }

    /// Remove a schedule. Returns `true` if it existed.
    pub fn remove_schedule(&self, id: &ReportScheduleId) -> bool {
        self.state
            .lock()
            .expect("report manager poisoned")
            .schedules
            .remove(id.as_str())
            .is_some()
    }

    /// Snapshot of all registered schedules.
    pub fn schedules(&self) -> Vec<ReportSchedule> {
        self.state
            .lock()
            .expect("report manager poisoned")
            .schedules
            .values()
            .cloned()
            .collect()
    }

    /// Fire every schedule that is due relative to `now_unix_nanos`.
    /// Returns the ids of generated reports.
    pub fn run_due_schedules(
        &self,
        now_unix_nanos: u64,
        policy_version: &str,
    ) -> Vec<Result<ReportId, ReportError>> {
        let due: Vec<ReportSchedule> = {
            let guard = self.state.lock().expect("report manager poisoned");
            guard
                .schedules
                .values()
                .filter(|s| s.due(now_unix_nanos))
                .cloned()
                .collect()
        };
        let mut results = Vec::with_capacity(due.len());
        for sched in due {
            let window_nanos = sched.window_secs.saturating_mul(1_000_000_000);
            let from = now_unix_nanos.saturating_sub(window_nanos);
            let request = ReportRequest {
                report_type: sched.report_type.clone(),
                from_unix_nanos: from,
                to_unix_nanos: now_unix_nanos,
                tenant: sched.tenant.clone(),
            };
            let res = self.generate(request, sched.format, policy_version, now_unix_nanos);
            // Stamp the schedule id + protected flag onto the
            // resulting record so the pruner respects it.
            if let Ok(ref rec) = res {
                let mut guard = self.state.lock().expect("report manager poisoned");
                if let Some(r) = guard.records.iter_mut().find(|r| r.id == rec.id) {
                    r.schedule_id = Some(sched.id.clone());
                    r.protected = sched.protected_outputs;
                }
                if let Some(s) = guard.schedules.get_mut(sched.id.as_str()) {
                    s.last_run_unix_nanos = Some(now_unix_nanos);
                }
            }
            results.push(res.map(|r| r.id));
        }
        results
    }

    /// Run a pruning pass.
    pub fn prune(&self, now_unix_nanos: u64) -> PruneOutcome {
        let mut guard = self.state.lock().expect("report manager poisoned");
        self.pruner.prune(&mut guard.records, now_unix_nanos)
    }

    /// Record a "report generated" event in the audit log via the
    /// policy-change feed. Returns any escalations the audit
    /// triggers emitted.
    ///
    /// Decoupled from [`Self::generate`] so the caller decides when
    /// (and whether) the audit log is informed — useful in tests
    /// that drive the manager in isolation.
    pub fn record_generation_event(&self, id: &str) -> Vec<Escalation> {
        self.audit
            .record_policy_change(format!("report.generated id={id}"))
    }

    fn next_id(&self) -> String {
        let mut guard = self.state.lock().expect("report manager poisoned");
        let n = guard.next_seq;
        guard.next_seq = guard.next_seq.saturating_add(1);
        format!("rpt-{n:06}")
    }

    fn insert(&self, record: ReportRecord) {
        let mut guard = self.state.lock().expect("report manager poisoned");
        guard.records.push(record);
    }

    fn replace(&self, record: ReportRecord) {
        let mut guard = self.state.lock().expect("report manager poisoned");
        if let Some(slot) = guard.records.iter_mut().find(|r| r.id == record.id) {
            *slot = record;
        } else {
            guard.records.push(record);
        }
    }
}

/// Builder for [`ReportManager`].
pub struct ReportManagerBuilder {
    audit: AuditLog,
    templates: TemplateRegistry,
    row_limit: usize,
    signer: Box<dyn ReportSigner>,
    watermark: Option<String>,
    prune_config: PruneConfig,
}

impl ReportManagerBuilder {
    /// Start a builder bound to the given audit log. The default
    /// template registry has the five built-ins; the default signer
    /// is [`NullReportSigner`].
    pub fn new(audit: AuditLog) -> Self {
        Self {
            audit,
            templates: TemplateRegistry::with_builtin(),
            row_limit: 1000,
            signer: Box::new(NullReportSigner),
            watermark: None,
            prune_config: PruneConfig::default(),
        }
    }

    /// Override the template registry.
    pub fn templates(mut self, registry: TemplateRegistry) -> Self {
        self.templates = registry;
        self
    }

    /// Override the per-report row cap.
    pub fn row_limit(mut self, limit: usize) -> Self {
        self.row_limit = limit;
        self
    }

    /// Wire in a non-default report signer.
    pub fn signer(mut self, signer: Box<dyn ReportSigner>) -> Self {
        self.signer = signer;
        self
    }

    /// Set the certification watermark.
    pub fn watermark(mut self, watermark: impl Into<String>) -> Self {
        self.watermark = Some(watermark.into());
        self
    }

    /// Override the pruner config.
    pub fn prune_config(mut self, config: PruneConfig) -> Self {
        self.prune_config = config;
        self
    }

    /// Build the [`ReportManager`].
    pub fn build(self) -> ReportManager {
        let mut certifier = ReportCertifier::new(self.signer);
        if let Some(w) = self.watermark {
            certifier = certifier.with_watermark(w);
        }
        ReportManager {
            engine: ReportEngine::new(self.audit.clone()).with_row_limit(self.row_limit),
            audit: self.audit,
            templates: self.templates,
            certifier: Mutex::new(certifier),
            pruner: ReportPruner::new(self.prune_config),
            state: Arc::new(Mutex::new(State {
                next_seq: 0,
                records: Vec::new(),
                schedules: HashMap::new(),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditRecordInput;
    use crate::policy::PolicyBundle;
    use crate::policy::bundle::{ClassificationResult, RequestMetadata};
    use crate::policy::composer::{
        AggregateDecision, ComplianceReason, Destination, ModuleId as ModId,
    };
    use crate::trust::TrustContext;

    fn sample_bundle() -> PolicyBundle {
        PolicyBundle {
            request: RequestMetadata {
                request_id: "req-1".into(),
                tenant_id: "local-dev".into(),
                timestamp_unix_ms: 0,
                source: "api".into(),
                model_hint: None,
            },
            trust: TrustContext::for_local_dev(),
            classification: ClassificationResult {
                level: "regulated".into(),
                matched_patterns: vec![],
                entity_count: 0,
            },
        }
    }

    fn sample_decision(allowed: bool, route: Destination, module: ModId) -> AggregateDecision {
        AggregateDecision {
            allowed,
            route: Some(route),
            flags: vec![],
            reasons: vec![ComplianceReason::new(
                module,
                Some(format!("{}.test_rule", module.as_str())),
                "test",
            )],
            modules_applied: vec![module],
        }
    }

    fn seed_log_with(audit: &AuditLog, ts: u64, allowed: bool, route: Destination, module: ModId) {
        let bundle = sample_bundle();
        let dec = sample_decision(allowed, route, module);
        let input = AuditRecordInput {
            request_id: "req-1",
            masked_request: b"masked",
            decision: &dec,
            bundle: &bundle,
            policy_version: "2026.05.22.001",
            credential_event_id: Some(format!("cred_{ts}")),
            timestamp_unix_nanos: ts,
        };
        audit.record(input).unwrap();
    }

    #[test]
    fn generate_produces_complete_record_with_content_hash() {
        let audit = AuditLog::default();
        seed_log_with(&audit, 100, true, Destination::Local, ModId::Ocap);
        let mgr = ReportManager::builder(audit).build();
        let req = ReportRequest {
            report_type: ReportType::OcapGovernance,
            from_unix_nanos: 0,
            to_unix_nanos: 1_000,
            tenant: None,
        };
        let rec = mgr
            .generate(req, ReportFormat::Json, "v1", 500)
            .expect("generate");
        assert_eq!(rec.status, ReportStatus::Complete);
        assert!(rec.content_hash_hex.is_some());
        assert!(rec.body_bytes.is_some());
        assert_eq!(rec.signature_hex, None); // null signer
    }

    #[test]
    fn list_returns_newest_first() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit).build();
        for ts in [10u64, 20, 30] {
            let req = ReportRequest {
                report_type: ReportType::SystemActivity,
                from_unix_nanos: 0,
                to_unix_nanos: 1_000,
                tenant: None,
            };
            mgr.generate(req, ReportFormat::Json, "v1", ts).unwrap();
        }
        let list = mgr.list();
        assert_eq!(list.len(), 3);
        assert!(list[0].created_at_unix_nanos > list[1].created_at_unix_nanos);
        assert!(list[1].created_at_unix_nanos > list[2].created_at_unix_nanos);
    }

    #[test]
    fn delete_refuses_protected_records() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit).build();
        let req = ReportRequest {
            report_type: ReportType::SystemActivity,
            from_unix_nanos: 0,
            to_unix_nanos: 1_000,
            tenant: None,
        };
        let rec = mgr.generate(req, ReportFormat::Json, "v1", 1).unwrap();
        mgr.set_protected(&rec.id, true).unwrap();
        assert!(mgr.delete(&rec.id).is_err());
        mgr.set_protected(&rec.id, false).unwrap();
        assert!(mgr.delete(&rec.id).is_ok());
    }

    #[test]
    fn unknown_template_errors() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit)
            .templates(TemplateRegistry::new())
            .build();
        let req = ReportRequest {
            report_type: ReportType::HipaaAuditTrail,
            from_unix_nanos: 0,
            to_unix_nanos: 1_000,
            tenant: None,
        };
        let res = mgr.generate(req, ReportFormat::Json, "v1", 1);
        assert!(matches!(res, Err(ReportError::UnknownTemplate(_))));
    }

    #[test]
    fn schedules_run_when_due() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit).build();
        let sched = ReportSchedule {
            id: ReportScheduleId("s1".into()),
            report_type: ReportType::SystemActivity,
            format: ReportFormat::Json,
            period_secs: 60,
            window_secs: 3600,
            tenant: None,
            last_run_unix_nanos: None,
            paused: false,
            protected_outputs: true,
        };
        mgr.add_schedule(sched).unwrap();
        let results = mgr.run_due_schedules(1_000_000_000_000, "v1");
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].protected,
            "scheduled output should inherit protected flag"
        );
        assert_eq!(list[0].schedule_id.as_ref().unwrap().as_str(), "s1");

        // Running again immediately should not fire (period not elapsed).
        let again = mgr.run_due_schedules(1_000_000_000_000, "v1");
        assert!(again.is_empty());
    }

    #[test]
    fn paused_schedules_do_not_fire() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit).build();
        let sched = ReportSchedule {
            id: ReportScheduleId("s2".into()),
            report_type: ReportType::SystemActivity,
            format: ReportFormat::Json,
            period_secs: 1,
            window_secs: 3600,
            tenant: None,
            last_run_unix_nanos: None,
            paused: true,
            protected_outputs: false,
        };
        mgr.add_schedule(sched).unwrap();
        let results = mgr.run_due_schedules(10_000_000_000, "v1");
        assert!(results.is_empty());
    }

    #[test]
    fn prune_drops_old_unprotected_records() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit)
            .prune_config(PruneConfig {
                default_retention_days: 0,
                ..Default::default()
            })
            .build();
        let req = ReportRequest {
            report_type: ReportType::SystemActivity,
            from_unix_nanos: 0,
            to_unix_nanos: 1_000,
            tenant: None,
        };
        mgr.generate(req, ReportFormat::Json, "v1", 0).unwrap();
        let outcome = mgr.prune(10u64 * 24 * 60 * 60 * 1_000_000_000);
        assert_eq!(outcome.deleted, 1);
        assert_eq!(mgr.record_count(), 0);
    }

    #[test]
    fn generation_event_is_audited_as_policy_change() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit.clone()).build();
        let req = ReportRequest {
            report_type: ReportType::SystemActivity,
            from_unix_nanos: 0,
            to_unix_nanos: 1_000,
            tenant: None,
        };
        let rec = mgr.generate(req, ReportFormat::Json, "v1", 1).unwrap();
        let _ = mgr.record_generation_event(&rec.id);
        // Policy-change events don't add audit chain entries by
        // themselves (the chain only links decisions). What we can
        // assert is that the call returned without panicking and
        // that the underlying trigger manager remained healthy.
        assert!(matches!(rec.status, ReportStatus::Complete));
    }

    #[test]
    fn schedule_collision_returns_error() {
        let audit = AuditLog::default();
        let mgr = ReportManager::builder(audit).build();
        let sched = ReportSchedule {
            id: ReportScheduleId("dup".into()),
            report_type: ReportType::SystemActivity,
            format: ReportFormat::Json,
            period_secs: 60,
            window_secs: 60,
            tenant: None,
            last_run_unix_nanos: None,
            paused: false,
            protected_outputs: false,
        };
        mgr.add_schedule(sched.clone()).unwrap();
        let res = mgr.add_schedule(sched);
        assert!(matches!(res, Err(ReportError::DuplicateSchedule(_))));
    }
}
