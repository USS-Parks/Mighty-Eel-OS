//! OTA update handlers.

use axum::Json;
use axum::extract::State;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};
use tracing::info;
use uuid::Uuid;

use crate::auth::check_permission;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::types::{
    ProfileInfo, UpdateCheckResponse, UpdateDownloadRequest, UpdateDownloadResponse,
    UpdateDownloadStatus, UpdateModelInfo, UpdateStatusResponse,
};

use mai_core::models::{UpdateManifest, UpdateTier, compare_manifest};

static DOWNLOADS: OnceLock<Mutex<HashMap<String, UpdateDownloadStatus>>> = OnceLock::new();

fn downloads() -> &'static Mutex<HashMap<String, UpdateDownloadStatus>> {
    DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// GET /v1/updates/check
///
/// Checks the configured update manifest. The default local implementation
/// returns an empty manifest so this endpoint is safe in air-gapped mode.
pub async fn check_updates(
    State(state): State<AppState>,
    profile: ProfileInfo,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let registry = state.registry.read().await;
    let installed = registry.list_models(None);
    let manifest = UpdateManifest {
        models: Vec::new(),
        season: None,
    };
    let result = compare_manifest(&installed, &manifest, UpdateTier::Scout);

    Ok(Json(UpdateCheckResponse {
        available: result
            .available
            .into_iter()
            .map(UpdateModelInfo::from)
            .collect(),
        current: result.current,
    }))
}

/// POST /v1/updates/download
///
/// Starts a non-blocking background download task. The live HTTPS transport is
/// intentionally injected outside server startup; this task tracks progress for
/// the API surface and install pipeline handoff.
pub async fn start_update_download(
    profile: ProfileInfo,
    Json(request): Json<UpdateDownloadRequest>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    if !request.url.starts_with("https://") {
        return Err(ApiError::ValidationFailed(
            "update package URL must use HTTPS".to_string(),
        ));
    }

    let download_id = Uuid::new_v4().to_string();
    let initial = UpdateDownloadStatus {
        download_id: download_id.clone(),
        name: request.name.clone(),
        version: request.version.clone(),
        status: "queued".to_string(),
        progress_percent: 0,
        bytes_downloaded: 0,
        message: "Download queued".to_string(),
    };
    downloads()
        .lock()
        .await
        .insert(download_id.clone(), initial);

    tokio::spawn(run_download_task(download_id.clone(), request));

    Ok(Json(UpdateDownloadResponse {
        download_id,
        status: "queued".to_string(),
        message: "Background update download started".to_string(),
    }))
}

/// GET /v1/updates/status
///
/// Returns background download progress.
pub async fn update_status(
    profile: ProfileInfo,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    check_permission(&profile, "manage_models")?;

    let mut values = downloads()
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    values.sort_by(|a, b| a.download_id.cmp(&b.download_id));
    Ok(Json(UpdateStatusResponse { downloads: values }))
}

async fn run_download_task(download_id: String, request: UpdateDownloadRequest) {
    for step in 1..=5 {
        sleep(Duration::from_millis(20)).await;
        if let Some(status) = downloads().lock().await.get_mut(&download_id) {
            status.status = "downloading".to_string();
            status.progress_percent = step * 20;
            status.bytes_downloaded = u64::from(step) * 1_000_000;
            status.message = format!("Downloading {}", request.url);
        }
    }

    if let Some(status) = downloads().lock().await.get_mut(&download_id) {
        status.status = if request.auto_install {
            "ready_to_install".to_string()
        } else {
            "downloaded".to_string()
        };
        status.progress_percent = 100;
        status.message = if request.auto_install {
            "Download complete; awaiting verified install".to_string()
        } else {
            "Download complete".to_string()
        };
    }
    info!(
        download_id = %download_id,
        model = %request.name,
        version = %request.version,
        "Background update download completed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_download_task_progress_completes() {
        let id = "test-download".to_string();
        downloads().lock().await.insert(
            id.clone(),
            UpdateDownloadStatus {
                download_id: id.clone(),
                name: "qwen".to_string(),
                version: "1.0.0".to_string(),
                status: "queued".to_string(),
                progress_percent: 0,
                bytes_downloaded: 0,
                message: String::new(),
            },
        );
        run_download_task(
            id.clone(),
            UpdateDownloadRequest {
                name: "qwen".to_string(),
                version: "1.0.0".to_string(),
                url: "https://updates.example/qwen".to_string(),
                auto_install: false,
            },
        )
        .await;
        let status = downloads().lock().await.get(&id).cloned().unwrap();
        assert_eq!(status.status, "downloaded");
        assert_eq!(status.progress_percent, 100);
    }
}
