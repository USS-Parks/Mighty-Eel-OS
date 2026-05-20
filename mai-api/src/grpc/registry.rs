//! MaiRegistry gRPC service implementation.
//!
//! Provides model registry queries with optional capability and status
//! filters. Registry write operations require Admin role.
//! The scan endpoint triggers re-registration from the vault.

use tonic::{Request, Response, Status};
use tracing::{debug, info};

use super::proto;
use super::{extract_grpc_profile, role_has_permission};
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

/// Convert a ModelSummary to a proto ModelDetail.
fn summary_to_proto(m: &mai_core::registry::ModelSummary) -> proto::ModelDetail {
    proto::ModelDetail {
        id: m.model_id.clone(),
        object: "model".to_string(),
        created: 0,
        owned_by: "island-mountain".to_string(),
        capabilities: Some(proto::ModelCapabilities {
            chat: m.capabilities.chat,
            completion: m.capabilities.completion,
            embedding: m.capabilities.embedding,
            vision: m.capabilities.vision,
            structured_output: m.capabilities.structured_output,
            max_context_tokens: m.capabilities.max_context_tokens,
        }),
        status: format!("{:?}", m.status),
        size_bytes: m.size_bytes,
        required_vram_bytes: 0,
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

        let models: Vec<proto::ModelDetail> =
            filtered.iter().map(|m| summary_to_proto(m)).collect();

        Ok(Response::new(proto::RegistryQueryResponse {
            total_models: models.len() as u32,
            loaded_models: loaded_count as u32,
            models,
        }))
    }

    /// Trigger a model scan. Admin only.
    ///
    /// NOTE: ModelRegistry does not yet have a scan() method.
    /// This is a placeholder that returns the current model count.
    /// Full filesystem scanning is deferred to Session 15 (Model Management).
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
        let count = all_models.len() as u32;

        Ok(Response::new(proto::ScanModelsResponse {
            models_found: count,
            new_models: 0,
            message: format!(
                "scan placeholder: {} models currently registered; \
                 full filesystem scan available in Session 15",
                count
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
