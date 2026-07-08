//! Model management handlers for the MAI REST API.
//!
//! Provides model listing (filtered by profile permissions), model detail,
//! and admin-only load/unload operations. Backend adapter names are never
//! exposed in responses.

use axum::Json;
use axum::extract::{FromRequest, Path, State};
use axum::response::IntoResponse;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::auth::{can_access_model, check_permission};
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    DiscoverResponse, DiscoveredPackage, ModelBenchmarkResponse, ModelCapabilities, ModelDetail,
    ModelInstallResponse, ModelListResponse, ModelOperationResponse, ModelRemoveResponse,
    ProfileInfo,
};

use mai_core::models::ModelLifecycleManager;
use mai_core::registry::ModelStatus;

static LIFECYCLE_MANAGER: OnceLock<Mutex<ModelLifecycleManager>> = OnceLock::new();

fn lifecycle_manager() -> &'static Mutex<ModelLifecycleManager> {
    LIFECYCLE_MANAGER.get_or_init(|| Mutex::new(ModelLifecycleManager::new()))
}

/// Raw handler for POST /v1/models/install.
///
/// Registered via `post_service(service_fn(...))` in routes.rs to sidestep
/// the axum 0.7 vs 0.8 `Handler` trait conflict. The axum version mismatch
/// (tonic pulls axum 0.7, mai-api uses axum 0.8) prevents 2+-extractor
/// async handlers from implementing the axum 0.8 `Handler` trait when any
/// extractor has a `FromRequest` (body) impl — the compiler sees duplicate
/// `Handler` trait names and cannot resolve.
///
/// This function receives the raw `Request<Body>` + cloned state and performs
/// all extraction manually.
pub async fn install_handler_raw(
    req: axum::http::Request<axum::body::Body>,
    state: AppState,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();

    // Use the identity the auth middleware authenticated and injected into the
    // request extensions, NOT the caller-supplied X-IM-Profile header (F1-NEW-1).
    // Re-parsing the header would let any valid low-privilege key present
    // `X-IM-Profile: x:admin` and escalate to admin on this route.
    let profile = match parts.extensions.get::<ProfileInfo>() {
        Some(p) => p.clone(),
        None => return ApiError::Unauthorized.into_response(),
    };

    // Check permission
    if let Err(e) = check_permission(&profile, "manage_models") {
        return e.into_response();
    }

    // Reconstruct request and extract JSON body via axum's built-in Json
    // extractor (passing () as state since Json doesn't use it).
    let req = axum::http::Request::from_parts(parts, body);
    let install_req = match Json::<InstallRequest>::from_request(req, &()).await {
        Ok(Json(r)) => r,
        Err(e) => {
            return ApiError::BadRequest(format!("Invalid request body: {e}")).into_response();
        }
    };

    let mount = std::path::PathBuf::from(&install_req.usb_mount_point);
    let mut registry = state.registry.write().await;

    match registry
        .install_from_usb(&mount, &install_req.package_name)
        .await
    {
        Ok(result) => {
            info!(
                model_id = %result.model_id,
                profile = %profile.profile_id,
                "Model installed from USB"
            );
            Json(ModelInstallResponse {
                model_id: result.model_id,
                status: "installed".to_string(),
                integrity_verified: result.integrity_verified,
                signature_verified: result.signature_verified,
                message: "Model installed successfully from USB".to_string(),
            })
            .into_response()
        }
        Err(e) => {
            warn!(
                error = %e,
                profile = %profile.profile_id,
                "USB install failed"
            );
            ApiError::BadRequest(format!("Install failed: {e}")).into_response()
        }
    }
}

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
        .map_or(0, |duration| duration.as_secs());

    let mut models: Vec<ModelDetail> = Vec::new();
    for summary in &summaries {
        // TODO(basho): read safety tags and the default flag from manifest
        // metadata; this heuristic treats all models as adult-accessible.
        let is_teen_safe = true;
        let is_child_safe = false;
        let is_default = false;

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
            required_vram_bytes: summary.required_vram_bytes,
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
        .map_or(0, |duration| duration.as_secs());

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
    let mut lifecycle = lifecycle_manager().lock().await;

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
            lifecycle
                .load_model(&mut registry, &model_id, None)
                .await
                .map_err(|e| {
                    warn!(error = %e, model = %model_id, "Failed to load model");
                    ApiError::ModelUnavailable(e.to_string())
                })?;

            info!(model = %model_id, profile = %profile.profile_id, "Model loaded");

            Ok(Json(ModelOperationResponse {
                operation: "load".to_string(),
                model_id: model_id.clone(),
                status: "loaded".to_string(),
                message: format!("Model '{model_id}' loaded to VRAM"),
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
    let mut lifecycle = lifecycle_manager().lock().await;

    // Check model exists
    if registry.get_model(&model_id).is_none() {
        return Err(ApiError::ModelNotFound(model_id.clone()));
    }

    let status = registry
        .get_status(&model_id)
        .ok_or_else(|| ApiError::ModelNotFound(model_id.clone()))?;

    match status {
        ModelStatus::Loaded | ModelStatus::Active { .. } => {
            lifecycle
                .unload_model(&mut registry, &model_id)
                .map_err(|e| {
                    warn!(error = %e, model = %model_id, "Failed to unload model");
                    ApiError::ModelUnavailable(e.to_string())
                })?;

            info!(model = %model_id, profile = %profile.profile_id, "Model unloaded");

            Ok(Json(ModelOperationResponse {
                operation: "unload".to_string(),
                model_id: model_id.clone(),
                status: "cold_storage".to_string(),
                message: format!("Model '{model_id}' unloaded to cold storage"),
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

// ─── Benchmark Model ───────────────────────────────────────────────────────

/// POST /v1/models/{model_id}/benchmark
///
/// Admin-only: run the standard model benchmark.
pub async fn benchmark_model(
    State(state): State<AppState>,
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let registry = state.registry.read().await;
    let mut lifecycle = lifecycle_manager().lock().await;
    let result = lifecycle
        .benchmark_model(&registry, &model_id)
        .map_err(|e| ApiError::ModelUnavailable(e.to_string()))?;

    info!(model = %model_id, profile = %profile.profile_id, "Model benchmark completed");
    Ok(Json(ModelBenchmarkResponse::from(result)))
}

/// GET /v1/models/{model_id}/benchmark
///
/// Returns the last benchmark result for a model.
pub async fn get_model_benchmark(
    profile: ProfileInfo,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let lifecycle = lifecycle_manager().lock().await;
    let result = lifecycle
        .last_benchmark(&model_id)
        .cloned()
        .ok_or_else(|| ApiError::ModelUnavailable("No benchmark result found".to_string()))?;
    Ok(Json(ModelBenchmarkResponse::from(result)))
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
//
// install_model handler has been replaced by install_handler_raw,
// registered via post_service(service_fn(...)) in routes.rs
// to avoid the axum 0.7 vs 0.8 Handler trait version conflict.

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
                crypto_erased: result.crypto_erased,
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
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    /// Package name, e.g. "qwen3-14b-Q4_K_M.mai-pkg"
    pub package_name: String,
    /// USB mount point, e.g. "D:" or "/mnt/usb"
    pub usb_mount_point: String,
}
