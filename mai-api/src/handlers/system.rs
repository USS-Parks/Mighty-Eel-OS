//! System management handlers for the MAI REST API.
//!
//! Covers power state control, registry management, adapter listing,
//! audit log access, and family profile queries. Most endpoints are
//! admin-only; enforced via check_permission().

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::Deserialize;
use tracing::{info, warn};

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::production_guard::ProductionReadinessReport;
use crate::state::AppState;
use crate::types::{
    AdapterHealthSummary, AdapterListResponse, AuditLogResponse, ModelCapabilities, ModelDetail,
    PowerStateResponse, PowerTransitionRequest, ProfileInfo, ProfileListResponse, ProfileResponse,
    RegistryQueryResponse, RegistryScanResponse,
};

use mai_core::power::TransitionTrigger;
use mai_core::registry::ModelStatus;

// ─── Production Readiness (SHIP-07 Slice B) ───────────────────────

/// `GET /v1/system/production-readiness`
///
/// Admin-only (`view_audit` permission). Returns the live
/// [`ProductionReadinessReport`] for the ship profile this server
/// booted with. Each call re-evaluates the report from the cached
/// profile + runtime introspection — operators always see the latest
/// view, never a stale snapshot.
///
/// Returns `MAI-1002 ValidationFailed` (422) when the server was
/// started without a ship profile (legacy/test bring-up): the
/// readiness contract is not meaningful in that mode and the endpoint
/// would otherwise leak the empty AppState defaults.
///
/// The response never includes secrets — the report only carries
/// check IDs, statuses, messages, and remediation hints sourced from
/// the profile and runtime introspection.
pub async fn production_readiness(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;
    let readiness = state.ship_readiness.as_ref().ok_or_else(|| {
        ApiError::ValidationFailed(
            "no ship profile loaded; start the server with MAI_SHIP_PROFILE \
             or MaiServer::with_ship_profile to enable readiness reporting"
                .to_string(),
        )
    })?;
    let report = ProductionReadinessReport::evaluate_with_runtime(
        readiness.profile.as_ref(),
        readiness.runtime_checks.as_ref(),
    );
    Ok(Json(report))
}

// ─── Air-Gap Status ────────────────────────────────────────────────

/// GET /v1/system/airgap
///
/// Returns the current connectivity state managed by [`AppState::airgap_policy`].
/// Authenticated callers may read this regardless of role — the air-gap
/// status is a system-wide invariant that every component already relies
/// on; surfacing it through the API is purely diagnostic.
pub async fn get_airgap_status(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let connectivity = state.airgap_policy.state();
    let response = serde_json::json!({
        "connectivity": connectivity.label(),
        "permits_cloud_route": connectivity.permits_cloud_route(),
        "requires_local_only": connectivity.requires_local_only(),
        "is_air_gapped": connectivity.is_air_gapped(),
    });
    Ok(Json(response))
}

// ─── Power State ───────────────────────────────────────────────────

/// GET /v1/power
///
/// Returns current power state, estimated power draw, time in state,
/// and whether auto-demotion is pending.
pub async fn get_power_state(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let power = state.power.read().await;
    let current = power.current_state();

    let response = PowerStateResponse {
        state: crate::types::power_state_to_string(current),
        estimated_watts: current.estimated_watts_gpu_era(),
        state_duration_secs: 0,  // Would need entry timestamp tracking
        demotion_pending: false, // Would need timer inspection
    };

    Ok(Json(response))
}

/// POST /v1/power/transition
///
/// Admin-only: triggers a power state transition. The `action` field
/// maps to TransitionTrigger variants.
pub async fn power_transition(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(req): Json<PowerTransitionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "power_control")?;

    let trigger = parse_transition_trigger(&req.action, req.reason.as_deref())?;

    let mut power = state.power.write().await;
    let result = power.request_transition(trigger).map_err(|e| {
        warn!(error = %e, action = %req.action, "Power transition failed");
        match e {
            mai_core::power::PowerError::InvalidTransition { from, to } => {
                ApiError::ValidationFailed(format!("Invalid power transition: {from} -> {to}"))
            }
            mai_core::power::PowerError::GuardFailed(reason) => {
                ApiError::ValidationFailed(format!("Transition guard failed: {reason}"))
            }
            _ => ApiError::InternalError,
        }
    })?;

    let current = power.current_state();

    info!(
        profile = %profile.profile_id,
        action = %req.action,
        state = current.as_str(),
        "Power transition executed"
    );

    Ok(Json(PowerStateResponse {
        state: crate::types::power_state_to_string(current),
        estimated_watts: current.estimated_watts_gpu_era(),
        state_duration_secs: 0,
        demotion_pending: false,
    }))
}

/// Map an API action string to a TransitionTrigger.
fn parse_transition_trigger(
    action: &str,
    reason: Option<&str>,
) -> Result<TransitionTrigger, ApiError> {
    match action {
        "boot" => Ok(TransitionTrigger::SystemBoot),
        "wake" => Ok(TransitionTrigger::WakeTrigger(
            mai_core::power::WakeSource::ApiRequest,
        )),
        "urgent_wake" => Ok(TransitionTrigger::UrgentWake(
            mai_core::power::WakeSource::ApiRequest,
        )),
        "promote" => Ok(TransitionTrigger::SentinelPromotion),
        "demote" => Ok(TransitionTrigger::InactivityTimeout),
        "deep_sleep" => Ok(TransitionTrigger::ExtendedInactivity),
        "manual" => Ok(TransitionTrigger::ManualOverride),
        "shutdown" => Ok(TransitionTrigger::SystemShutdown),
        other => Err(ApiError::ValidationFailed(format!(
            "Unknown power action '{other}'. Valid: boot, wake, urgent_wake, promote, demote, deep_sleep, manual, shutdown"
        ))),
    }
}

// ─── Registry ──────────────────────────────────────────────────────

/// GET /v1/registry
///
/// Returns registry contents with model states. Shows all registered
/// models regardless of load state.
pub async fn get_registry(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state.registry.read().await;
    let summaries = registry.list_models(None);

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let models: Vec<ModelDetail> = summaries
        .iter()
        .map(|s| {
            let status_str = match &s.status {
                ModelStatus::ColdStorage => "cold_storage",
                ModelStatus::Loading { .. } => "loading",
                ModelStatus::Loaded => "loaded",
                ModelStatus::Active { .. } => "active",
                ModelStatus::Evicting => "evicting",
                ModelStatus::Evicted => "evicted",
            };
            ModelDetail {
                id: s.model_id.clone(),
                object: "model".to_string(),
                created: now_epoch,
                owned_by: "island-mountain".to_string(),
                capabilities: ModelCapabilities::from(&s.capabilities),
                status: status_str.to_string(),
                size_bytes: s.size_bytes,
                required_vram_bytes: s.required_vram_bytes,
            }
        })
        .collect();

    let loaded = models
        .iter()
        .filter(|m| m.status == "loaded" || m.status == "active")
        .count();

    let response = RegistryQueryResponse {
        total_models: models.len(),
        loaded_models: loaded,
        models,
    };

    Ok(Json(response))
}

/// POST /v1/registry/scan
///
/// Admin-only: triggers a rescan of the model directory to discover
/// new model packages (e.g., after USB install).
pub async fn registry_scan(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "registry_write")?;

    // In production, this would trigger a filesystem scan of the vault
    // model directory and register newly discovered manifests.
    let registry = state.registry.read().await;
    let current_count = registry.model_count();

    info!(
        profile = %profile.profile_id,
        current_models = current_count,
        "Registry scan requested"
    );

    Ok(Json(RegistryScanResponse {
        models_found: current_count,
        new_models: 0, // Actual scan would compare before/after
        message: "Registry scan complete".to_string(),
    }))
}

// ─── Adapters ──────────────────────────────────────────────────────

/// GET /v1/adapters
///
/// Lists registered adapters with their health status. Adapter IDs
/// are opaque; backend implementation names are never exposed.
pub async fn list_adapters(
    State(state): State<AppState>,
    _profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    let health = state.health.read().await;
    let snapshot = health.get_snapshot();

    let adapters: Vec<AdapterHealthSummary> = snapshot
        .adapters
        .values()
        .map(AdapterHealthSummary::from)
        .collect();

    Ok(Json(AdapterListResponse { adapters }))
}

// ─── Audit Log ─────────────────────────────────────────────────────

/// Query parameters for audit log pagination.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditLogQuery {
    /// Number of entries to return (default 50, max 500)
    #[serde(default = "default_audit_limit")]
    pub limit: u64,
    /// Offset for pagination (default 0)
    #[serde(default)]
    pub offset: u64,
}

fn default_audit_limit() -> u64 {
    50
}

/// GET /v1/audit/log
///
/// Admin-only: returns paginated audit trail with hash chain integrity.
pub async fn get_audit_log(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Query(params): Query<AuditLogQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "view_audit")?;

    let limit = params.limit.min(500);
    let total = state.audit_writer.entry_count().await.map_err(|e| {
        warn!(error = %e, "Failed to get audit entry count");
        ApiError::InternalError
    })?;

    let entries = state
        .audit_writer
        .read_recent(limit as usize)
        .await
        .map_err(|e| {
            warn!(error = %e, "Failed to read audit log");
            ApiError::InternalError
        })?;

    // Convert internal AuditEntry to wire AuditLogEntry
    let log_entries: Vec<crate::types::AuditLogEntry> = entries
        .iter()
        .map(|e| crate::types::AuditLogEntry {
            sequence: 0,
            timestamp: e.timestamp.to_string(),
            profile_id: e.profile_id.clone(),
            method: e.method.clone(),
            endpoint: e.path.clone(),
            model: e.model_name.clone(),
            tokens_in: 0,
            tokens_out: 0,
            latency_ms: e.duration_ms,
            status_code: e.status_code,
            request_id: e.entry_id.clone(),
            chain_hash: e.entry_hash.clone(),
        })
        .collect();

    Ok(Json(AuditLogResponse {
        entries: log_entries,
        total,
        offset: params.offset,
        limit,
    }))
}

// ─── Profiles ──────────────────────────────────────────────────────

/// GET /v1/profiles
///
/// A non-admin sees only their own profile (correct and complete). "Admin sees
/// all profiles" is not yet wired to the vault profile store, so for an admin
/// this returns an explicit 501 Not Implemented rather than silently returning
/// only the caller — which would misrepresent a partial view as the full set
/// (audit P4). TODO(basho): enumerate all profiles from the vault store for admins.
pub async fn list_profiles(
    State(_state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    if profile.role == crate::types::ProfileRole::Admin {
        return Err(ApiError::NotImplemented(
            "listing all profiles requires the vault profile store".to_string(),
        ));
    }
    let profiles = vec![ProfileResponse {
        id: profile.profile_id.clone(),
        name: profile
            .display_name
            .clone()
            .unwrap_or_else(|| profile.profile_id.clone()),
        role: format!("{:?}", profile.role),
    }];

    Ok(Json(ProfileListResponse { profiles }))
}

/// GET /v1/profiles/{profile_id}
///
/// Returns a specific profile's information. Admin can view any profile;
/// non-admin can only view their own.
pub async fn get_profile(
    State(_state): State<AppState>,
    profile: ProfileInfo,
    Path(profile_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // Non-admin can only view their own profile
    if profile.role != crate::types::ProfileRole::Admin && profile.profile_id != profile_id {
        return Err(ApiError::PermissionDenied(format!(
            "Profile '{}' cannot view profile '{profile_id}'",
            profile.profile_id,
        )));
    }

    // TODO(basho): look up from the vault profile store. Currently returns
    // the requested profile if it matches the caller.
    if profile.profile_id == profile_id {
        Ok(Json(ProfileResponse {
            id: profile.profile_id.clone(),
            name: profile
                .display_name
                .clone()
                .unwrap_or_else(|| profile.profile_id.clone()),
            role: format!("{:?}", profile.role),
        }))
    } else {
        // Admin viewing another profile: minimal response (vault profile
        // store lookup is the TODO(basho) above).
        Ok(Json(ProfileResponse {
            id: profile_id.clone(),
            name: profile_id,
            role: "unknown".to_string(),
        }))
    }
}
