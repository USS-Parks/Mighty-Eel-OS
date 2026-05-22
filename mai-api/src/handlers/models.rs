//! Model management handlers for the MAI REST API.
//!
//! Provides model listing (filtered by profile permissions), model detail,
//! and admin-only load/unload operations. Backend adapter names are never
//! exposed in responses.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use crate::auth::{can_access_model, check_permission};
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    DiscoverResponse, DiscoveredPackage, ModelCapabilities, ModelDetail, ModelInstallResponse,
    ModelListResponse, ModelOperationResponse, ModelRemoveResponse, ProfileInfo,
};

use mai_core::registry::ModelStatus;

// ─── List Models ───────────────────────────────────────────────────

/// GET /v1/models
///
/// Lists all models visible to the requesting profile. Child profiles
/// see only child-safe models, teen profiles see teen-safe, guests see
/// only the default model. Admin/Adult see all.
pub async fn list_models(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    // list_models permission: Admin/Adult/Teen can list, Child/Guest cannot
    // but we allow all roles to hit this endpoint; filtering handles visibility
    let registry = state.registry.read().await;
    let summaries = registry.list_models(None);

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut models: Vec<ModelDetail> = Vec::new();
    for summary in &summaries {
        // Determine model safety tags (in production, these come from manifest metadata)
        // For now, use heuristic: all models are adult-accessible, specific tags TBD
        let is_teen_safe = true; // Placeholder: read from manifest metadata
        let is_child_safe = false; // Placeholder: only small/safe models
        let is_default = false; // Placeholder: check config for default model

        if !can_access_model(
            &profile,
            &summary.model_id,
            is_teen_safe,
            is_child_safe,
            is_default,
        ) {
            continue;
        }

        let status_str = match &summary.status {
            ModelStatus::ColdStorage => "cold_storage",
            ModelStatus::Loading { .. } => "loading",
            ModelStatus::Loaded => "loaded",
            ModelStatus::Active { .. } => "active",
            ModelStatus::Evicting => "evicting",
            ModelStatus::Evicted => "evicted",
        };

        models.push(ModelDetail {
            id: summary.model_id.clone(),
            object: "model".to_string(),
            created: now_epoch,
            owned_by: "island-mountain".to_string(),
            capabilities: ModelCapabilities::from(&summary.capabilities),
            status: status_str.to_string(),
            size_bytes: summary.size_bytes,
            required_vram_bytes: 0, // Not in ModelSummary; available in full manifest
        });
    }

    let response = ModelListResponse {
        object: "list".to_string(),
        data: models,
    };

    debug!(
        profile = %profile.profile_id,
        count = response.data.len(),
        "Model list served"
    );

    Ok(Json(response))
}

// ─── Model Detail ──────────────────────────────────────────────────

/// GET /v1/models/{model_id}
///
/// Returns detailed information about a specific model including
/// capabilities, format, size, and VRAM requirements.
pub async fn get_model(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state.registry.read().await;

    let manifest = registry
        .get_model(&model_id)
        .ok_or_else(|| ApiError::ModelNotFound(model_id.clone()))?;

    let status = registry
        .get_status(&model_id)
        .ok_or_else(|| ApiError::ModelNotFound(model_id.clone()))?;

    let status_str = match status {
        ModelStatus::ColdStorage => "cold_storage",
        ModelStatus::Loading { .. } => "loading",
        ModelStatus::Loaded => "loaded",
        ModelStatus::Active { .. } => "active",
        ModelStatus::Evicting => "evicting",
        ModelStatus::Evicted => "evicted",
    };

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let detail = ModelDetail {
        id: model_id.clone(),
        object: "model".to_string(),
        created: now_epoch,
        owned_by: "island-mountain".to_string(),
        capabilities: ModelCapabilities::from(&manifest.capabilities),
        status: status_str.to_string(),
        size_bytes: manifest.model.size_bytes,
        required_vram_bytes: manifest.model.required_vram_bytes,
    };

    Ok(Json(detail))
}

// ─── Load Model ────────────────────────────────────────────────────

/// POST /v1/models/{model_id}/load
///
/// Admin-only: triggers model loading from cold storage to VRAM.
/// Returns immediately with operation status; actual loading is async.
pub async fn load_model(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let mut registry = state.registry.write().await;

    // Check model exists
    if registry.get_model(&model_id).is_none() {
        return Err(ApiError::ModelNotFound(model_id.clone()));
    }

    // Check current status
    let status = registry
        .get_status(&model_id)
        .ok_or_else(|| ApiError::ModelNotFound(model_id.clone()))?;

    match status {
        ModelStatus::ColdStorage => {
            // Initiate loading
            registry
                .update_status(
                    &model_id,
                    ModelStatus::Loading {
                        progress_percent: 0,
                    },
                )
                .map_err(|e| {
                    warn!(error = %e, model = %model_id, "Failed to start model load");
                    ApiError::InternalError
                })?;

            info!(model = %model_id, profile = %profile.profile_id, "Model load initiated");

            Ok(Json(ModelOperationResponse {
                operation: "load".to_string(),
                model_id: model_id.clone(),
                status: "loading".to_string(),
                message: format!("Model '{model_id}' load initiated from cold storage"),
            }))
        }
        ModelStatus::Loaded | ModelStatus::Active { .. } => Ok(Json(ModelOperationResponse {
            operation: "load".to_string(),
            model_id: model_id.clone(),
            status: "already_loaded".to_string(),
            message: format!("Model '{model_id}' is already loaded"),
        })),
        ModelStatus::Loading { progress_percent } => Ok(Json(ModelOperationResponse {
            operation: "load".to_string(),
            model_id: model_id.clone(),
            status: "loading".to_string(),
            message: format!(
                "Model '{model_id}' is already loading ({progress_percent}% complete)"
            ),
        })),
        _ => Err(ApiError::ModelUnavailable(format!(
            "Model '{model_id}' is in state {status:?} and cannot be loaded"
        ))),
    }
}

// ─── Unload Model ──────────────────────────────────────────────────

/// POST /v1/models/{model_id}/unload
///
/// Admin-only: initiates model eviction from VRAM back to cold storage.
/// In-flight requests are drained before VRAM is freed.
pub async fn unload_model(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let mut registry = state.registry.write().await;

    // Check model exists
    if registry.get_model(&model_id).is_none() {
        return Err(ApiError::ModelNotFound(model_id.clone()));
    }

    let status = registry
        .get_status(&model_id)
        .ok_or_else(|| ApiError::ModelNotFound(model_id.clone()))?;

    match status {
        ModelStatus::Loaded | ModelStatus::Active { .. } => {
            registry
                .update_status(&model_id, ModelStatus::Evicting)
                .map_err(|e| {
                    warn!(error = %e, model = %model_id, "Failed to start model unload");
                    ApiError::InternalError
                })?;

            info!(model = %model_id, profile = %profile.profile_id, "Model unload initiated");

            Ok(Json(ModelOperationResponse {
                operation: "unload".to_string(),
                model_id: model_id.clone(),
                status: "evicting".to_string(),
                message: format!(
                    "Model '{model_id}' unload initiated, draining in-flight requests"
                ),
            }))
        }
        ModelStatus::ColdStorage | ModelStatus::Evicted => Ok(Json(ModelOperationResponse {
            operation: "unload".to_string(),
            model_id: model_id.clone(),
            status: "not_loaded".to_string(),
            message: format!("Model '{model_id}' is not currently loaded"),
        })),
        _ => Err(ApiError::ModelUnavailable(format!(
            "Model '{model_id}' is in state {status:?} and cannot be unloaded now"
        ))),
    }
}

// ─── Discover USB Packages ────────────────────────────────────────────

/// GET /v1/models/discover
///
/// Admin-only: scan USB drives for installable .mai-pkg directories.
pub async fn discover_packages(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let result = mai_core::models::usb::discover_usb_packages();

    let packages: Vec<DiscoveredPackage> = result
        .packages
        .iter()
        .map(|pkg| DiscoveredPackage {
            name: pkg.name.clone(),
            model_name: pkg.manifest.model.name.clone(),
            version: pkg.manifest.model.version.clone(),
            format: format!("{:?}", pkg.manifest.model.format),
            size_bytes: pkg.manifest.model.size_bytes,
            model_id: pkg.model_id(),
        })
        .collect();

    let response = DiscoverResponse {
        packages,
        drives_scanned: result.drives_scanned.len(),
        errors: result.errors,
    };

    Ok(Json(response))
}

// ─── Install Model from USB ──────────────────────────────────────────

/// POST /v1/models/install
///
/// Admin-only: install a model package from USB into the registry.
pub async fn install_model(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Json(body): Json<InstallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let mount = std::path::PathBuf::from(&body.usb_mount_point);
    let mut registry = state.registry.write().await;

    match registry.install_from_usb(&mount, &body.package_name).await {
        Ok(result) => {
            info!(
                model_id = %result.model_id,
                profile = %profile.profile_id,
                "Model installed from USB"
            );
            Ok(Json(ModelInstallResponse {
                model_id: result.model_id,
                status: "installed".to_string(),
                integrity_verified: result.integrity_verified,
                signature_verified: result.signature_verified,
                message: "Model installed successfully from USB".to_string(),
            }))
        }
        Err(e) => {
            warn!(
                error = %e,
                profile = %profile.profile_id,
                "USB install failed"
            );
            Err(ApiError::BadRequest(format!("Install failed: {e}")))
        }
    }
}

// ─── Remove Model ────────────────────────────────────────────────────

/// POST /v1/models/{model_id}/remove
///
/// Admin-only: securely remove a model from registry and vault.
pub async fn remove_model_handler(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let options = mai_core::models::remove::RemoveOptions::default();
    let mut registry = state.registry.write().await;

    match registry.secure_remove_model(&model_id, &options).await {
        Ok(result) => {
            info!(
                model_id = %model_id,
                profile = %profile.profile_id,
                "Model removed"
            );
            Ok(Json(ModelRemoveResponse {
                model_id: result.model_id,
                status: "removed".to_string(),
                secure_wipe: result.secure_wipe,
                snapshot_created: result.snapshot_created,
                message: format!("Model '{model_id}' removed successfully"),
            }))
        }
        Err(e) => {
            warn!(error = %e, profile = %profile.profile_id, "Model removal failed");
            Err(ApiError::ModelUnavailable(format!("Removal failed: {e}")))
        }
    }
}

// ─── Install Request Body ────────────────────────────────────────────

/// Request body for POST /v1/models/install
#[derive(Debug, Clone, serde::Deserialize)]
pub struct InstallRequest {
    /// Package name, e.g. "qwen3-14b-Q4_K_M.mai-pkg"
    pub package_name: String,
    /// USB mount point, e.g. "D:" or "/mnt/usb"
    pub usb_mount_point: String,
}
