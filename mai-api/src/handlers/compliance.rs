//! Compliance management handlers.
//!
//! Binds the [`mai_compliance`] façades — `PolicyManager`,
//! `AuditLog`, `ReportManager`, and the `AuditFeed` — onto the public
//! REST surface so the compliance dashboard (and the SDK
//! `client.compliance.*` namespace) can drive every operation an
//! operator, regulator, or acquirer needs.
//!
//! Route groups:
//!
//! - `GET  /v1/compliance/status`               — composer + module health
//! - `GET  /v1/compliance/policies`             — list per-module status
//! - `GET  /v1/compliance/policies/{module}`    — single module status
//! - `POST /v1/compliance/policies/reload`      — re-apply the active template
//! - `POST /v1/compliance/policies/template`    — apply a named template
//! - `POST /v1/compliance/modules/{name}/enable`  — flip a module on
//! - `POST /v1/compliance/modules/{name}/disable` — flip a module off
//! - `GET  /v1/compliance/audit`                — query the chain
//! - `GET  /v1/compliance/audit/{id}`           — one entry by id
//! - `GET  /v1/compliance/audit/verify`         — verify the full chain
//! - `GET  /v1/compliance/audit/integrity`      — cheap integrity snapshot
//! - `GET  /v1/compliance/reports`              — list every report record
//! - `POST /v1/compliance/reports/generate`     — synchronous generation
//! - `GET  /v1/compliance/reports/{id}`         — one record (no body)
//! - `GET  /v1/compliance/reports/{id}/download` — rendered body bytes
//! - `DELETE /v1/compliance/reports/{id}`       — delete (refuses protected)
//! - `GET  /v1/compliance/feed`                 — SSE event stream

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Interval;
use tracing::{info, warn};

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::ProfileInfo;

use mai_compliance::audit::{
    AuditQuery, AuditQueryRow, IntegrityStatus, RoutingDecision, VerificationStatus,
};
use mai_compliance::bundle::BundleVerifier;
use mai_compliance::policy::{
    AuditFeed, FeedEvent, FeedSubscriber, ModuleId, OverallStatus, PolicyTemplate,
};
use mai_compliance::reports::{ReportError, ReportFormat, ReportRecord, ReportRequest, ReportType};

// ─── Wire helpers ──────────────────────────────────────────────────

/// Parse a module identifier from a URL path segment. Accepts the
/// canonical wire form (`hipaa`, `itar`, `ocap`) and is case-insensitive.
fn parse_module_id(raw: &str) -> Result<ModuleId, ApiError> {
    match raw.to_ascii_lowercase().as_str() {
        "hipaa" => Ok(ModuleId::Hipaa),
        "itar" | "itar_ear" | "ear" => Ok(ModuleId::Itar),
        "ocap" => Ok(ModuleId::Ocap),
        other => Err(ApiError::ValidationFailed(format!(
            "Unknown compliance module '{other}'. Expected one of: hipaa, itar, ocap"
        ))),
    }
}

fn parse_template(raw: &str) -> Result<PolicyTemplate, ApiError> {
    match raw.to_ascii_lowercase().as_str() {
        "standard" => Ok(PolicyTemplate::Standard),
        "healthcare" => Ok(PolicyTemplate::Healthcare),
        "defense" => Ok(PolicyTemplate::Defense),
        "tribal_government" | "tribalgovernment" => Ok(PolicyTemplate::TribalGovernment),
        other => Err(ApiError::ValidationFailed(format!(
            "Unknown policy template '{other}'. Expected one of: standard, healthcare, defense, tribal_government"
        ))),
    }
}

fn parse_report_type(raw: &str) -> ReportType {
    match raw.to_ascii_lowercase().as_str() {
        "hipaa_audit_trail" | "hipaa" => ReportType::HipaaAuditTrail,
        "itar_compliance_summary" | "itar" | "ear" => ReportType::ItarComplianceSummary,
        "ocap_governance" | "ocap" => ReportType::OcapGovernance,
        "system_activity" | "activity" => ReportType::SystemActivity,
        "monthly_digest" | "digest" => ReportType::MonthlyDigest,
        other => ReportType::Custom(other.to_string()),
    }
}

fn parse_report_format(raw: Option<&str>) -> Result<ReportFormat, ApiError> {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        None | Some("json") => Ok(ReportFormat::Json),
        Some("html") => Ok(ReportFormat::Html),
        Some("csv") => Ok(ReportFormat::Csv),
        Some("text") => Ok(ReportFormat::Text),
        Some(other) => Err(ApiError::ValidationFailed(format!(
            "Unknown report format '{other}'. Expected one of: json, html, csv, text"
        ))),
    }
}

fn parse_routing_decision(raw: &str) -> Result<RoutingDecision, ApiError> {
    match raw.to_ascii_lowercase().as_str() {
        "allow" => Ok(RoutingDecision::Allow),
        "local_only" | "local_only_allowed" => Ok(RoutingDecision::LocalOnly),
        "quarantine" => Ok(RoutingDecision::Quarantine),
        "deny" => Ok(RoutingDecision::Deny),
        other => Err(ApiError::ValidationFailed(format!(
            "Unknown decision '{other}'. Expected one of: allow, local_only, quarantine, deny"
        ))),
    }
}

fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn map_report_error(e: ReportError) -> ApiError {
    warn!(error = %e, "report manager error");
    match e {
        ReportError::NotFound(id) => ApiError::ModelNotFound(format!("report {id}")),
        ReportError::UnknownTemplate(t) => {
            ApiError::ValidationFailed(format!("Unknown report template '{}'", t.as_str()))
        }
        ReportError::DuplicateSchedule(id) => {
            ApiError::ValidationFailed(format!("Schedule id already exists: {id}"))
        }
        ReportError::Engine(inner) => {
            ApiError::ValidationFailed(format!("Report engine error: {inner}"))
        }
        ReportError::Certify(_) => ApiError::InternalError,
    }
}

// ─── Policy handlers ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ModuleStatusWire {
    pub module: String,
    pub enabled: bool,
    pub priority: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PolicyListResponse {
    pub modules: Vec<ModuleStatusWire>,
}

fn module_to_wire(s: &mai_compliance::policy::ModuleStatus) -> ModuleStatusWire {
    ModuleStatusWire {
        module: s.module.as_str().to_string(),
        enabled: s.enabled,
        priority: s.priority,
    }
}

/// `GET /v1/compliance/policies`
pub async fn list_policies(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let modules = state
        .policy_manager
        .list_policies()
        .iter()
        .map(module_to_wire)
        .collect();
    Ok(Json(PolicyListResponse { modules }))
}

/// `GET /v1/compliance/policies/{module}`
pub async fn get_policy(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(module): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let module = parse_module_id(&module)?;
    let status = state.policy_manager.module_status(module);
    Ok(Json(module_to_wire(&status)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleToggle {
    pub enabled: bool,
}

/// `PUT /v1/compliance/policies/{module}`
///
/// Admin-only. Body: `{"enabled": true|false}`. Flips the module's
/// enabled flag and publishes a `ModuleStateChanged` feed event.
pub async fn update_policy(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(module): Path<String>,
    Json(req): Json<ModuleToggle>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let module = parse_module_id(&module)?;
    let was_enabled = state.policy_manager.module_status(module).enabled;
    let now_enabled = state.policy_manager.set_module_enabled(module, req.enabled);
    let changed = was_enabled != now_enabled;
    info!(
        profile = %profile.profile_id,
        module = %module.as_str(),
        enabled = now_enabled,
        changed,
        "Compliance module toggle"
    );
    Ok(Json(serde_json::json!({
        "module": module.as_str(),
        "enabled": now_enabled,
        "changed": changed,
    })))
}

/// `POST /v1/compliance/policies/reload`
///
/// Admin-only. Re-applies the current composer config; bumps the
/// reload counter and invalidates the decision cache.
pub async fn reload_policy(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let cfg = state.policy_manager.composer_config();
    state.policy_manager.reload(cfg);
    let overall = state.policy_manager.overall_status();
    info!(
        profile = %profile.profile_id,
        reloads = overall.reload_count,
        "Compliance policy reload"
    );
    Ok(Json(serde_json::json!({
        "reload_count": overall.reload_count,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyTemplateRequest {
    pub template: String,
}

/// `POST /v1/compliance/policies/template`
///
/// Admin-only. Swap the active composer config for one of the
/// pre-built templates (Standard / Healthcare / Defense /
/// TribalGovernment).
pub async fn apply_template(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<ApplyTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let template = parse_template(&req.template)?;
    state.policy_manager.apply_template(template);
    info!(
        profile = %profile.profile_id,
        template = %req.template,
        "Compliance template applied"
    );
    Ok(Json(serde_json::json!({
        "template": req.template,
    })))
}

#[derive(Debug, Serialize)]
pub struct ComplianceStatusResponse {
    pub modules: Vec<ModuleStatusWire>,
    pub priority: Vec<String>,
    pub reload_count: u64,
    pub audit_integrity: IntegrityWire,
    pub subscribers: usize,
}

#[derive(Debug, Serialize)]
pub struct IntegrityWire {
    pub entry_count: u64,
    pub chain_count: u64,
    pub head_hash: String,
    pub last_verify: String,
    pub last_verify_error: Option<String>,
}

fn integrity_to_wire(status: IntegrityStatus) -> IntegrityWire {
    IntegrityWire {
        entry_count: status.entry_count,
        chain_count: status.chain_count,
        head_hash: status.head_hash,
        last_verify: verification_label(status.last_verify).to_string(),
        last_verify_error: status.last_verify_error,
    }
}

fn verification_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Unknown => "unknown",
        VerificationStatus::Verified => "verified",
        VerificationStatus::Tampered => "tampered",
    }
}

/// `GET /v1/compliance/status`
pub async fn compliance_status(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let overall: OverallStatus = state.policy_manager.overall_status();
    let modules = overall.modules.iter().map(module_to_wire).collect();
    let priority = overall
        .priority
        .iter()
        .map(|m| m.as_str().to_string())
        .collect();
    let integrity = integrity_to_wire(state.compliance_audit.integrity_status());
    let subscribers = state.policy_manager.audit_feed().subscriber_count();
    Ok(Json(ComplianceStatusResponse {
        modules,
        priority,
        reload_count: overall.reload_count,
        audit_integrity: integrity,
        subscribers,
    }))
}

/// `POST /v1/compliance/modules/{name}/enable`
pub async fn enable_module(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let module = parse_module_id(&name)?;
    state.policy_manager.enable_module(module);
    Ok(Json(serde_json::json!({
        "module": module.as_str(),
        "enabled": true,
    })))
}

/// `POST /v1/compliance/modules/{name}/disable`
pub async fn disable_module(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let module = parse_module_id(&name)?;
    state.policy_manager.disable_module(module);
    Ok(Json(serde_json::json!({
        "module": module.as_str(),
        "enabled": false,
    })))
}

// ─── Audit handlers ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditQueryParams {
    #[serde(default)]
    pub from: Option<u64>,
    #[serde(default)]
    pub to: Option<u64>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AuditRowWire {
    pub entry: serde_json::Value,
    pub status: String,
}

impl From<AuditQueryRow> for AuditRowWire {
    fn from(row: AuditQueryRow) -> Self {
        Self {
            entry: serde_json::to_value(&row.entry).unwrap_or(serde_json::Value::Null),
            status: verification_label(row.status).to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuditQueryResponse {
    pub rows: Vec<AuditRowWire>,
    pub total: usize,
}

/// `GET /v1/compliance/audit`
pub async fn query_audit(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Query(params): Query<AuditQueryParams>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let module = params.module.as_deref().map(parse_module_id).transpose()?;
    let decision = params
        .decision
        .as_deref()
        .map(parse_routing_decision)
        .transpose()?;
    let limit = params.limit.map(|l| l.min(1000));
    let query = AuditQuery {
        from: params.from,
        to: params.to,
        module,
        decision,
        tenant: params.tenant,
        limit,
    };
    let rows = state.compliance_audit.query(&query);
    let total = rows.len();
    let rows: Vec<AuditRowWire> = rows.into_iter().map(AuditRowWire::from).collect();
    Ok(Json(AuditQueryResponse { rows, total }))
}

/// `GET /v1/compliance/audit/{id}`
pub async fn get_audit_entry(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    state
        .compliance_audit
        .get(id)
        .map(|row| Json(AuditRowWire::from(row)))
        .ok_or_else(|| ApiError::ModelNotFound(format!("audit entry {id}")))
}

/// `GET /v1/compliance/audit/integrity`
pub async fn audit_integrity(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let status = state.compliance_audit.integrity_status();
    Ok(Json(integrity_to_wire(status)))
}

/// `GET /v1/compliance/audit/verify`
///
/// Runs the full chain verifier. Returns `{"verified": true|false,
/// "error": Option<String>}` so the dashboard can render a badge.
pub async fn verify_audit(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    // SAFETY: `BundleVerifier` is bounded `Send + Sync` in AppState.
    let chain_verifier: &(dyn BundleVerifier + Send + Sync) = state.bundle_verifier.as_ref();
    let outcome = state
        .compliance_audit
        .verify_full(Some(&VerifierAdapter(chain_verifier)));
    let (chain_ok, chain_err) = match outcome {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };
    Ok(Json(serde_json::json!({
        "verified": chain_ok,
        "error": chain_err,
    })))
}

/// Type-erasing wrapper so the trait-object verifier satisfies
/// [`AuditLog::verify_full`]'s `V: BundleVerifier` bound.
struct VerifierAdapter<'a>(&'a (dyn BundleVerifier + Send + Sync));

impl BundleVerifier for VerifierAdapter<'_> {
    fn verify(
        &self,
        payload_hash: &[u8; 32],
        signature_bytes: &[u8],
        public_key_id: &str,
    ) -> Result<(), mai_compliance::bundle::BundleError> {
        self.0.verify(payload_hash, signature_bytes, public_key_id)
    }
}

// ─── Report handlers ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ReportRecordWire {
    pub id: String,
    pub report_type: String,
    pub status: String,
    pub output_format: String,
    pub from_unix_nanos: u64,
    pub to_unix_nanos: u64,
    pub tenant: Option<String>,
    pub created_at_unix_nanos: u64,
    pub completed_at_unix_nanos: Option<u64>,
    pub content_hash_hex: Option<String>,
    pub signature_hex: Option<String>,
    pub error: Option<String>,
    pub protected: bool,
    pub schedule_id: Option<String>,
}

impl From<&ReportRecord> for ReportRecordWire {
    fn from(r: &ReportRecord) -> Self {
        Self {
            id: r.id.clone(),
            report_type: r.request.report_type.as_str().to_string(),
            status: format!("{:?}", r.status).to_lowercase(),
            output_format: r.output_format.as_str().to_string(),
            from_unix_nanos: r.request.from_unix_nanos,
            to_unix_nanos: r.request.to_unix_nanos,
            tenant: r.request.tenant.clone(),
            created_at_unix_nanos: r.created_at_unix_nanos,
            completed_at_unix_nanos: r.completed_at_unix_nanos,
            content_hash_hex: r.content_hash_hex.clone(),
            signature_hex: r.signature_hex.clone(),
            error: r.error.clone(),
            protected: r.protected,
            schedule_id: r.schedule_id.as_ref().map(|s| s.0.clone()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReportListResponse {
    pub reports: Vec<ReportRecordWire>,
    pub total: usize,
}

/// `GET /v1/compliance/reports`
pub async fn list_reports(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let records = state.report_manager.list();
    let reports: Vec<ReportRecordWire> = records.iter().map(ReportRecordWire::from).collect();
    let total = reports.len();
    Ok(Json(ReportListResponse { reports, total }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateReportRequest {
    pub report_type: String,
    pub from_unix_nanos: u64,
    pub to_unix_nanos: u64,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default = "default_policy_version")]
    pub policy_version: String,
}

fn default_policy_version() -> String {
    "local-dev".to_string()
}

/// `POST /v1/compliance/reports/generate`
pub async fn generate_report(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<GenerateReportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    if req.to_unix_nanos < req.from_unix_nanos {
        return Err(ApiError::ValidationFailed(
            "to_unix_nanos must be >= from_unix_nanos".to_string(),
        ));
    }
    let report_type = parse_report_type(&req.report_type);
    let format = parse_report_format(req.format.as_deref())?;
    let request = ReportRequest {
        report_type,
        from_unix_nanos: req.from_unix_nanos,
        to_unix_nanos: req.to_unix_nanos,
        tenant: req.tenant,
    };
    let record = state
        .report_manager
        .generate(request, format, &req.policy_version, now_unix_nanos())
        .map_err(map_report_error)?;
    Ok(Json(ReportRecordWire::from(&record)))
}

/// `GET /v1/compliance/reports/{id}`
pub async fn get_report(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let record = state
        .report_manager
        .get(&id)
        .ok_or_else(|| ApiError::ModelNotFound(format!("report {id}")))?;
    Ok(Json(ReportRecordWire::from(&record)))
}

/// `GET /v1/compliance/reports/{id}/download`
///
/// Streams the rendered body bytes with the format's MIME type. Falls
/// back to 404 when the record exists but has been pruned out of
/// memory (`body_bytes == None`).
pub async fn download_report(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    check_permission(&profile, "view_audit")?;
    let record = state
        .report_manager
        .get(&id)
        .ok_or_else(|| ApiError::ModelNotFound(format!("report {id}")))?;
    let bytes = record
        .body_bytes
        .ok_or_else(|| ApiError::ModelUnavailable(format!("report {id} body not available")))?;
    let content_type = record.output_format.content_type();
    let filename = format!("{}.{}", record.id, record.output_format.as_str());
    let disposition = format!("attachment; filename=\"{filename}\"");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

/// `DELETE /v1/compliance/reports/{id}`
pub async fn delete_report(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;
    let record = state.report_manager.delete(&id).map_err(map_report_error)?;
    Ok(Json(ReportRecordWire::from(&record)))
}

// ─── SSE feed ──────────────────────────────────────────────────────

/// `GET /v1/compliance/feed`
///
/// Server-sent events stream of [`FeedEvent`]s drained from the
/// PolicyManager's `AuditFeed`. Each event is emitted as a JSON-
/// encoded SSE record with the `event:` field set to the kind
/// (`decision_made`, `policy_changed`, `module_state_changed`,
/// `violation_detected`).
///
/// Polls the subscriber buffer at 250 ms intervals. Adds a 15 s
/// keep-alive comment so intermediaries (and `EventSource` clients)
/// keep the connection open during idle stretches.
pub async fn compliance_feed(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let feed: AuditFeed = state.policy_manager.audit_feed();
    let subscriber = feed.subscribe();
    let interval = tokio::time::interval(Duration::from_millis(250));
    let stream = FeedStream {
        subscriber,
        interval,
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Adapts a [`FeedSubscriber`] poll loop into an SSE stream.
pub struct FeedStream {
    subscriber: FeedSubscriber,
    interval: Interval,
}

impl Stream for FeedStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // First, drain any buffered event without waiting.
        if let Some(event) = self.subscriber.pop() {
            return Poll::Ready(Some(Ok(feed_event_to_sse(&event))));
        }
        // Otherwise wait for the next tick and re-poll.
        match self.interval.poll_tick(cx) {
            Poll::Ready(_) => {
                if let Some(event) = self.subscriber.pop() {
                    Poll::Ready(Some(Ok(feed_event_to_sse(&event))))
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn feed_event_to_sse(event: &FeedEvent) -> Event {
    let kind = event.kind();
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    Event::default().event(kind).data(data)
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_module_ids() {
        assert!(matches!(parse_module_id("hipaa"), Ok(ModuleId::Hipaa)));
        assert!(matches!(parse_module_id("ITAR"), Ok(ModuleId::Itar)));
        assert!(matches!(parse_module_id("ear"), Ok(ModuleId::Itar)));
        assert!(matches!(parse_module_id("ocap"), Ok(ModuleId::Ocap)));
        assert!(parse_module_id("phi").is_err());
    }

    #[test]
    fn parses_templates() {
        assert!(matches!(
            parse_template("standard"),
            Ok(PolicyTemplate::Standard)
        ));
        assert!(matches!(
            parse_template("HEALTHCARE"),
            Ok(PolicyTemplate::Healthcare)
        ));
        assert!(matches!(
            parse_template("tribal_government"),
            Ok(PolicyTemplate::TribalGovernment)
        ));
        assert!(parse_template("unknown").is_err());
    }

    #[test]
    fn parses_report_types_and_formats() {
        assert!(matches!(
            parse_report_type("hipaa"),
            ReportType::HipaaAuditTrail
        ));
        assert!(matches!(
            parse_report_type("ocap"),
            ReportType::OcapGovernance
        ));
        match parse_report_type("custom_audit") {
            ReportType::Custom(s) => assert_eq!(s, "custom_audit"),
            _ => panic!("expected custom variant"),
        }
        assert!(matches!(parse_report_format(None), Ok(ReportFormat::Json)));
        assert!(matches!(
            parse_report_format(Some("html")),
            Ok(ReportFormat::Html)
        ));
        assert!(parse_report_format(Some("pdf")).is_err());
    }

    #[test]
    fn parses_routing_decisions() {
        assert!(matches!(
            parse_routing_decision("allow"),
            Ok(RoutingDecision::Allow)
        ));
        assert!(matches!(
            parse_routing_decision("deny"),
            Ok(RoutingDecision::Deny)
        ));
        assert!(matches!(
            parse_routing_decision("local_only"),
            Ok(RoutingDecision::LocalOnly)
        ));
        assert!(parse_routing_decision("escalate").is_err());
    }
}
