//! MaiRegistry gRPC service implementation.
//!
//! Provides model registry queries with optional capability and status
//! filters. Registry write operations require Admin role.
//! The scan endpoint triggers re-registration from the vault.

use tonic::{Request, Response, Status};
use tracing::{debug, info};

use super::proto;
use super::{extract_grpc_profile, model_summary_to_proto_detail, role_has_permission};
use crate::state::AppState;

use mai_core::registry::ModelFilter;

/// MaiRegistry service implementation.
pub struct MaiRegistryService {
    state: AppState,
}

impl MaiRegistryService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl proto::mai_registry_server::MaiRegistry for MaiRegistryService {
    /// Query the model registry with optional filters.
    async fn query_registry(
        &self,
        request: Request<proto::RegistryQueryRequest>,
    ) -> Result<Response<proto::RegistryQueryResponse>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
        if !role_has_permission(&role, "list_models") {
            return Err(Status::permission_denied(
                "profile lacks list_models permission",
            ));
        }

        let req = request.into_inner();
        debug!(
            profile_id = %profile_id,
            capability_filter = %req.capability_filter,
            status_filter = %req.status_filter,
            "gRPC QueryRegistry"
        );

        // Build ModelFilter from request parameters
        let filter = if req.capability_filter.is_empty() {
            None
        } else {
            Some(ModelFilter {
                backend_compatible_with: None,
                requires_capability: Some(req.capability_filter.clone()),
                max_vram_bytes: None,
            })
        };

        let registry = self.state.registry.read().await;
        let all_models = registry.list_models(filter.as_ref());

        // Apply status filter client-side (ModelFilter doesn't have status)
        let filtered: Vec<_> = all_models
            .iter()
            .filter(|m| {
                if req.status_filter.is_empty() || req.status_filter == "all" {
                    return true;
                }
                let status_str = format!("{:?}", m.status).to_lowercase();
                status_str == req.status_filter.to_lowercase()
            })
            .collect();

        let loaded_count = filtered
            .iter()
            .filter(|m| format!("{:?}", m.status).to_lowercase() == "loaded")
            .count();

        let models: Vec<proto::ModelDetail> = filtered
            .iter()
            .map(|m| model_summary_to_proto_detail(m, 0))
            .collect();

        #[allow(clippy::cast_possible_truncation)]
        let total_models = models.len() as u32;
        #[allow(clippy::cast_possible_truncation)]
        let loaded_models = loaded_count as u32;

        Ok(Response::new(proto::RegistryQueryResponse {
            total_models,
            loaded_models,
            models,
        }))
    }

    /// Trigger a model scan. Admin only.
    ///
    /// TODO(basho): ModelRegistry does not yet have a scan() method; this
    /// returns the current model count until filesystem scanning lands.
    async fn scan_models(
        &self,
        request: Request<proto::ScanModelsRequest>,
    ) -> Result<Response<proto::ScanModelsResponse>, Status> {
        let (profile_id, role) = extract_grpc_profile(&request)?;
        if !role_has_permission(&role, "registry_write") {
            return Err(Status::permission_denied(
                "admin role required for registry scan",
            ));
        }

        info!(profile_id = %profile_id, "gRPC ScanModels (placeholder)");

        let registry = self.state.registry.read().await;
        let all_models = registry.list_models(None);
        #[allow(clippy::cast_possible_truncation)]
        let count = all_models.len() as u32;

        Ok(Response::new(proto::ScanModelsResponse {
            models_found: count,
            new_models: 0,
            message: format!(
                "scan placeholder: {count} models currently registered; \
                 full filesystem scan not yet implemented"
            ),
        }))
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_service_constructable() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MaiRegistryService>();
    }
}
